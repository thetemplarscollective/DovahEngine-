//! M17 probe: what items exist in the Sleeping Giant Inn, and what
//! do their records actually say?
//!
//! Answers, from the real Skyrim.esm (no guessing):
//! 1. Every loose pickupable ref in the inn (MISC/WEAP/ARMO/ALCH/
//!    BOOK/INGR/AMMO/KEYM/SLGM/NOTE/SCRL bases) — EDID, base form
//!    ID, position. Plus counts by base type.
//! 2. Position collisions: any two placed refs (any type) sharing
//!    an exact position — the pickup despawn matches entities by
//!    position, so a collision involving a pickupable ref would
//!    despawn the wrong thing.
//! 3. CONT containers in the inn and their CNTO contents (form ID,
//!    count, content base type/EDID) — including LVLI entries,
//!    which need leveled-list resolution (future work).
//! 4. Item DATA layouts, verified against known items: IronSword
//!    (WEAP value/weight/damage), IronArmor/Helmet/Boots/Gauntlets
//!    (ARMO value/weight/rating/slots), Tankard (MISC),
//!    SweetRoll (ALCH food?).
//! 5. Player head: does FaceGen 00000007.nif exist? Any generic
//!    male head NIF in the Meshes BSAs?
//!
//! Usage: cargo run -p dovah-app --example item_probe
//! (run from the workspace root; reads test-data/Skyrim.esm and
//! test-data/Skyrim - Meshes0/1.bsa).

use dovah_bsa::BsaArchive;
use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::{HashMap, HashSet};

const ESM_PATH: &str = "test-data/Skyrim.esm";

const PICKUPABLE: &[&str] = &[
    "MISC", "WEAP", "ARMO", "ALCH", "BOOK", "INGR", "AMMO", "KEYM", "SLGM", "NOTE", "SCRL",
];

fn parse_placement(data: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    if data.len() != 24 {
        return None;
    }
    let f = |i: usize| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    Some(([f(0), f(1), f(2)], [f(3), f(4), f(5)]))
}

fn form_id_from_field(record: &Record, field_type: &[u8; 4]) -> Option<u32> {
    let field = record.field(field_type)?;
    if field.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(field.data[0..4].try_into().unwrap()))
}

/// All records filed under one cell's child groups.
fn cell_children(plugin: &Plugin, cell_id: u32) -> Vec<&Record> {
    let mut out = Vec::new();
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        if matches!(
            g.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        ) && g.label_form_id() == Some(cell_id)
        {
            out.extend(g.records());
        }
        stack.extend(g.subgroups());
    }
    out
}

fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn dump_item(record: &Record, by_form_id: &HashMap<u32, &Record>) {
    let edid = record.edid().unwrap_or_default();
    println!(
        "--- {} 0x{:08x} ({}) ---",
        edid,
        record.form_id,
        record.type_str()
    );
    for code in [b"DATA", b"DNAM", b"BODT", b"BOD2", b"ENIT", b"DESC"] {
        if let Some(f) = record.field(code) {
            println!(
                "  {} ({} B): {}",
                String::from_utf8_lossy(code),
                f.data.len(),
                hex(&f.data)
            );
        }
    }
    // Common prefix: value i32 + weight f32.
    if let Some(f) = record.field(b"DATA") {
        if f.data.len() >= 8 {
            let value = i32::from_le_bytes(f.data[0..4].try_into().unwrap());
            let weight = f32::from_le_bytes(f.data[4..8].try_into().unwrap());
            println!("  value={value} weight={weight}");
        }
        // WEAP damage: u16 at offset 8 (to verify).
        if record.type_str() == "WEAP" && f.data.len() >= 10 {
            let dmg = u16::from_le_bytes(f.data[8..10].try_into().unwrap());
            println!("  damage@8={dmg}");
        }
    }
    if let Some(f) = record.field(b"DNAM") {
        if f.data.len() >= 4 {
            let rating = f32::from_le_bytes(f.data[0..4].try_into().unwrap());
            println!("  armor rating (DNAM f32)={rating}");
        }
    }
    if let Some(f) = record.field(b"BODT") {
        if f.data.len() >= 4 {
            let slots = u32::from_le_bytes(f.data[0..4].try_into().unwrap());
            println!("  BODT slots=0x{slots:08x}");
        }
    }
    if let Some(f) = record.field(b"BOD2") {
        if f.data.len() >= 4 {
            let slots = u32::from_le_bytes(f.data[0..4].try_into().unwrap());
            println!("  BOD2 slots=0x{slots:08x}");
        }
    }
    // ARMO -> ARMA models (the worn meshes).
    if record.type_str() == "ARMO" {
        for f in record.fields(b"MODL") {
            if f.data.len() == 4 {
                let id = u32::from_le_bytes(f.data[0..4].try_into().unwrap());
                if let Some(arma) = by_form_id.get(&id) {
                    let race = form_id_from_field(arma, b"RNAM").unwrap_or(0);
                    let male = arma.field(b"MOD2").map(|f| f.as_str()).unwrap_or_default();
                    let female = arma.field(b"MOD3").map(|f| f.as_str()).unwrap_or_default();
                    println!("  ARMA 0x{id:08x} race=0x{race:08x} M={male} F={female}");
                }
            }
        }
    }
    // Ground model.
    if let Some(m) = record.field(b"MODL").map(|f| f.as_str()) {
        if record.type_str() != "ARMO" {
            println!("  MODL={m}");
        }
    }
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for r in plugin.all_records() {
        by_form_id.insert(r.form_id, r);
    }
    let mut by_edid: HashMap<String, &Record> = HashMap::new();
    for r in plugin.all_records() {
        if let Some(e) = r.edid() {
            by_edid.insert(e, r);
        }
    }

    // --- 1+2. Inn refs: pickupables + position collisions ---
    let inn = by_edid.get("RiverwoodSleepingGiantInn").expect("inn CELL");
    let children = cell_children(&plugin, inn.form_id);
    println!("inn children: {}", children.len());
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut pickupables: Vec<(&Record, &Record, [f32; 3])> = Vec::new();
    let mut pos_map: HashMap<[u32; 3], Vec<(u32, String, String)>> = HashMap::new();
    for r in &children {
        let kind = r.type_str();
        if kind != "REFR" && kind != "ACHR" {
            continue;
        }
        let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) else {
            continue;
        };
        let Some(base_id) = form_id_from_field(r, b"NAME") else {
            continue;
        };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        let base_type = base.type_str().to_string();
        let edid = base.edid().unwrap_or_default();
        *counts.entry(base_type.clone()).or_default() += 1;
        let key = [pos[0].to_bits(), pos[1].to_bits(), pos[2].to_bits()];
        pos_map
            .entry(key)
            .or_default()
            .push((r.form_id, base_type.clone(), edid.clone()));
        if PICKUPABLE.contains(&base_type.as_str()) {
            pickupables.push((r, base, pos));
        }
    }
    println!("\n== pickupable refs in the inn: {} ==", pickupables.len());
    let mut type_counts: HashMap<String, usize> = HashMap::new();
    for (r, base, pos) in &pickupables {
        *type_counts.entry(base.type_str()).or_default() += 1;
        println!(
            "  ref 0x{:08x} {:<5} {} pos=({:.0},{:.0},{:.0})",
            r.form_id,
            base.type_str(),
            base.edid().unwrap_or_default(),
            pos[0],
            pos[1],
            pos[2]
        );
    }
    println!("pickupable by type: {type_counts:?}");
    println!("all placed refs by base type: {counts:?}");

    println!("\n== position collisions ==");
    let mut collisions = 0;
    for (pos, refs) in &pos_map {
        if refs.len() > 1 {
            let involves_pickupable = refs
                .iter()
                .any(|(_, t, _)| PICKUPABLE.contains(&t.as_str()));
            println!(
                "  pos {:?}: {} refs (pickupable involved: {})",
                pos.iter().map(|b| f32::from_bits(*b)).collect::<Vec<_>>(),
                refs.len(),
                involves_pickupable
            );
            for (id, t, e) in refs {
                println!("    ref 0x{id:08x} {t} {e}");
            }
            collisions += 1;
        }
    }
    if collisions == 0 {
        println!("  none — every placed ref has a unique position");
    }

    // --- 3. CONT containers in the inn ---
    println!("\n== CONT containers in the inn ==");
    for r in &children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(base_id) = form_id_from_field(r, b"NAME") else {
            continue;
        };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        if base.type_str() != "CONT" {
            continue;
        }
        let pos = r
            .field(b"DATA")
            .and_then(|f| parse_placement(&f.data))
            .map(|(p, _)| p);
        println!(
            "  ref 0x{:08x} {} pos={pos:?}",
            r.form_id,
            base.edid().unwrap_or_default()
        );
        for f in base.fields(b"CNTO") {
            if f.data.len() != 8 {
                println!("    CNTO: {} B (unexpected size)", f.data.len());
                continue;
            }
            let item_id = u32::from_le_bytes(f.data[0..4].try_into().unwrap());
            let count = i32::from_le_bytes(f.data[4..8].try_into().unwrap());
            match by_form_id.get(&item_id) {
                Some(item) => println!(
                    "    CNTO: {}x {} 0x{:08x} ({})",
                    count,
                    item.edid().unwrap_or_default(),
                    item_id,
                    item.type_str()
                ),
                None => println!("    CNTO: {count}x <missing 0x{item_id:08x}>"),
            }
        }
    }

    // --- 4. Item DATA layouts ---
    println!("\n== item field layouts ==");
    for edid in [
        "IronSword",
        "IronDagger",
        "IronArmor",
        "IronHelmet",
        "IronBoots",
        "IronGauntlets",
        "IronShield",
        "Tankard",
        "SweetRoll",
        "FoodBread01",
        "Lockpick",
        "ClothesFarm01",
        "PotionMinorHealing",
    ] {
        match by_edid.get(edid) {
            Some(rec) => dump_item(rec, &by_form_id),
            None => println!("--- {edid}: NOT FOUND ---"),
        }
    }

    // --- 5. Player head ---
    println!("\n== player head ==");
    let mut found_facegen = false;
    let mut head_candidates: HashSet<String> = HashSet::new();
    for bsa in [
        "test-data/Skyrim - Meshes0.bsa",
        "test-data/Skyrim - Meshes1.bsa",
    ] {
        match BsaArchive::open(bsa) {
            Ok(archive) => {
                for stored in archive.list() {
                    let lower = stored.to_lowercase();
                    if lower.ends_with("facegeom/skyrim.esm/00000007.nif") {
                        println!("  FaceGen 00000007.nif EXISTS in {bsa}: {stored}");
                        found_facegen = true;
                    }
                    if lower.contains("actors/character")
                        && lower.contains("head")
                        && !lower.contains("facegen")
                        && lower.ends_with(".nif")
                    {
                        head_candidates.insert(stored.to_string());
                    }
                }
            }
            Err(e) => println!("  could not open {bsa}: {e}"),
        }
    }
    if !found_facegen {
        println!("  FaceGen 00000007.nif does NOT exist in Meshes0/1");
    }
    println!(
        "  generic head-ish NIFs under actors/character ({}):",
        head_candidates.len()
    );
    let mut heads: Vec<_> = head_candidates.into_iter().collect();
    heads.sort();
    for h in heads.iter().take(30) {
        println!("    {h}");
    }
}
