//! M17 probe, part 2: ARMO layouts (real EDIDs), LVLI resolution
//! data, ALCH food flag, and the player head (correct BSA names).
//!
//! Usage: cargo run -p dovah-app --example item_probe2

use dovah_bsa::BsaArchive;
use dovah_esm::{Plugin, Record};
use std::collections::{HashMap, HashSet};

const ESM_PATH: &str = "test-data/Skyrim.esm";

fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn form_id_from_field(record: &Record, field_type: &[u8; 4]) -> Option<u32> {
    let field = record.field(field_type)?;
    if field.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(field.data[0..4].try_into().unwrap()))
}

fn dump_arma(record: &Record, by_form_id: &HashMap<u32, &Record>) {
    if record.type_str() != "ARMO" {
        return;
    }
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

fn dump_item(record: &Record, by_form_id: &HashMap<u32, &Record>) {
    let edid = record.edid().unwrap_or_default();
    println!(
        "--- {} 0x{:08x} ({}) ---",
        edid,
        record.form_id,
        record.type_str()
    );
    for code in [b"DATA", b"DNAM", b"BODT", b"BOD2", b"ENIT"] {
        if let Some(f) = record.field(code) {
            println!(
                "  {} ({} B): {}",
                String::from_utf8_lossy(code),
                f.data.len(),
                hex(&f.data)
            );
        }
    }
    if let Some(f) = record.field(b"DATA") {
        if f.data.len() >= 8 {
            let value = i32::from_le_bytes(f.data[0..4].try_into().unwrap());
            let weight = f32::from_le_bytes(f.data[4..8].try_into().unwrap());
            println!("  value={value} weight={weight}");
        }
    }
    if record.type_str() == "ARMO" {
        if let Some(f) = record.field(b"DNAM") {
            if f.data.len() >= 4 {
                println!(
                    "  armor rating (DNAM f32)={}",
                    f32::from_le_bytes(f.data[0..4].try_into().unwrap())
                );
            }
        }
        for code in [b"BODT", b"BOD2"] {
            if let Some(f) = record.field(code) {
                if f.data.len() >= 4 {
                    println!(
                        "  {} slots=0x{:08x}",
                        String::from_utf8_lossy(code),
                        u32::from_le_bytes(f.data[0..4].try_into().unwrap())
                    );
                }
            }
        }
        dump_arma(record, by_form_id);
    }
    if record.type_str() == "ALCH" {
        if let Some(f) = record.field(b"ENIT") {
            if f.data.len() >= 4 {
                println!(
                    "  ENIT flags=0x{:08x}",
                    u32::from_le_bytes(f.data[0..4].try_into().unwrap())
                );
            }
        }
    }
    if let Some(m) = record.field(b"MODL").map(|f| f.as_str()) {
        if record.type_str() != "ARMO" {
            println!("  MODL={m}");
        }
    }
}

fn dump_lvli(record: &Record, by_form_id: &HashMap<u32, &Record>) {
    let edid = record.edid().unwrap_or_default();
    println!("--- LVLI {} 0x{:08x} ---", edid, record.form_id);
    if let Some(f) = record.field(b"LVLD") {
        println!("  LVLD ({} B): {}", f.data.len(), hex(&f.data));
        if f.data.len() >= 4 {
            let flags = f.data[0];
            let chance_none = f.data[1];
            println!("  flags=0x{flags:02x} (bit0=calc-all<=PC? bit1=use-all?) chance_none={chance_none}");
        }
    }
    if let Some(f) = record.field(b"LVLG") {
        println!("  LVLG (global): {}", hex(&f.data));
    }
    let mut n = 0;
    for f in record.fields(b"LVLO") {
        n += 1;
        if f.data.len() != 12 {
            println!("  LVLO: {} B (unexpected)", f.data.len());
            continue;
        }
        let level = i16::from_le_bytes(f.data[0..2].try_into().unwrap());
        let _unk = i16::from_le_bytes(f.data[2..4].try_into().unwrap());
        let item_id = u32::from_le_bytes(f.data[4..8].try_into().unwrap());
        let count = i16::from_le_bytes(f.data[8..10].try_into().unwrap());
        match by_form_id.get(&item_id) {
            Some(item) => println!(
                "  LVLO: lvl {level} count {count} {} 0x{item_id:08x} ({})",
                item.edid().unwrap_or_default(),
                item.type_str()
            ),
            None => println!("  LVLO: lvl {level} count {count} <missing 0x{item_id:08x}>"),
        }
    }
    println!("  {n} LVLO entries");
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

    println!("== ARMO (real EDIDs) ==");
    for edid in [
        "ArmorIronCuirass",
        "ArmorIronHelmet",
        "ArmorIronBoots",
        "ArmorIronGauntlets",
        "ArmorIronShield",
        "ClothesFarmShirt01",
        "ClothesFarmPants01",
        "ClothesFarmBoots01",
    ] {
        match by_edid.get(edid) {
            Some(rec) => dump_item(rec, &by_form_id),
            None => println!("--- {edid}: NOT FOUND ---"),
        }
    }

    println!("\n== ALCH food vs potion ==");
    for edid in [
        "FoodBread01A",
        "FoodSweetRoll",
        "FoodApple",
        "FoodCheeseWheel01A",
        "RestoreHealth01",
        "RestoreStamina02",
        "FoodWineAlto",
        "Gold001",
    ] {
        match by_edid.get(edid) {
            Some(rec) => dump_item(rec, &by_form_id),
            None => println!("--- {edid}: NOT FOUND ---"),
        }
    }

    println!("\n== BOOK ==");
    for edid in ["Book2CommonGreatWar", "Book3ValuableBlades"] {
        match by_edid.get(edid) {
            Some(rec) => dump_item(rec, &by_form_id),
            None => println!("--- {edid}: NOT FOUND ---"),
        }
    }

    println!("\n== LVLI ==");
    for edid in [
        "LItemBarrelFoodSame70",
        "LItemFoodInnCommon",
        "LItemInnRuralDrink",
        "LootGoldChange",
        "LItemMiscVendorClothing75",
    ] {
        match by_edid.get(edid) {
            Some(rec) => dump_lvli(rec, &by_form_id),
            None => println!("--- {edid}: NOT FOUND ---"),
        }
    }

    println!("\n== player head ==");
    let mut found_facegen = false;
    let mut head_candidates: HashSet<String> = HashSet::new();
    for bsa in [
        "test-data/Skyrim_-_Meshes0.bsa",
        "test-data/Skyrim_-_Meshes1.bsa",
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
    let mut heads: Vec<_> = head_candidates.into_iter().collect();
    heads.sort();
    println!(
        "  generic head-ish NIFs under actors/character ({}):",
        heads.len()
    );
    for h in heads.iter().take(40) {
        println!("    {h}");
    }

    // The player record itself: race/sex/outfit.
    println!("\n== player NPC_ 0x00000007 ==");
    match by_form_id.get(&0x0000_0007) {
        Some(npc) => {
            println!("  type={} edid={:?}", npc.type_str(), npc.edid());
            for f in &npc.fields {
                let code = String::from_utf8_lossy(&f.field_type).into_owned();
                if code == "RNAM" || code == "DOFT" || code == "ACBS" {
                    println!("  {code}: {}", hex(&f.data));
                }
            }
        }
        None => println!("  NPC_ 0x00000007 not in Skyrim.esm"),
    }
}
