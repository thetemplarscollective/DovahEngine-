//! Melee probe (melee milestone): what does the real data say
//! about weapons, NPC health, and the inn's actors?
//!
//! A. WEAP records by EDID (IronSword, IronDagger, IronWarAxe,
//!    IronMace, SteelSword, IronGreatsword, HuntingBow,
//!    IronBattleaxe): FULL, DATA bytes (value/weight/damage),
//!    DNAM bytes (full hex dump + f32/u32 reads — speed and
//!    reach live here somewhere), ETYP, CRDT.
//! B. NPC_ records for the inn's ACHR refs (Delphine, Orgnar,
//!    Embry + any others in CELL 0x000133c6): EDID, FULL, ACBS
//!    hex (level lives here), CNAM (class form ID), RNAM (race).
//! C. CLAS records for the classes found in B: FULL, DATA hex
//!    (the health formula's class side, if it lives here).
//! D. Player starting outfit: NPC_ 0x00000007 DOFT -> OTFT INAM
//!    entries — record types (is there a WEAP in the starting
//!    inventory?).
//! E. WEAP bases among the inn cell's REFRs (loose weapons the
//!    player can pick up) + ACHR positions in the inn (proof
//!    cameras for the melee renders).
//!
//! Usage: cargo run -p dovah-esm --example melee_probe
//! (run from the workspace root; reads test-data/)

use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::HashMap;
use std::path::Path;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const INN_CELL: u32 = 0x000133c6;
const PLAYER_NPC: u32 = 0x00000007;

const WEAPONS: &[&str] = &[
    "IronSword",
    "IronDagger",
    "IronWarAxe",
    "IronMace",
    "SteelSword",
    "IronGreatsword",
    "HuntingBow",
    "IronBattleaxe",
    "WoodenSword",
];

fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn f32_at(data: &[u8], off: usize) -> Option<f32> {
    let b: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
    Some(f32::from_le_bytes(b))
}

fn u32_at(data: &[u8], off: usize) -> Option<u32> {
    let b: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(b))
}

fn u16_at(data: &[u8], off: usize) -> Option<u16> {
    let b: [u8; 2] = data.get(off..off + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(b))
}

fn i16_at(data: &[u8], off: usize) -> Option<i16> {
    let b: [u8; 2] = data.get(off..off + 2)?.try_into().ok()?;
    Some(i16::from_le_bytes(b))
}

fn field_str(record: &Record, ty: &[u8; 4]) -> Option<String> {
    record.field(ty).map(|f| {
        String::from_utf8_lossy(&f.data)
            .trim_end_matches('\0')
            .to_string()
    })
}

fn main() {
    let plugin = Plugin::open(Path::new(ESM_PATH)).expect("load Skyrim.esm");
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    let mut by_edid: HashMap<String, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
        if let Some(edid) = record.edid() {
            by_edid.insert(edid, record);
        }
    }

    // ---- A. WEAP records ----
    println!("=== A. WEAP ===");
    for edid in WEAPONS {
        let Some(r) = by_edid.get(*edid) else {
            println!("{edid}: NOT FOUND");
            continue;
        };
        if r.type_str() != "WEAP" {
            println!("{edid}: type {} (not WEAP)", r.type_str());
            continue;
        }
        let full = field_str(r, b"FULL").unwrap_or_default();
        let data = r.field(b"DATA").map(|f| f.data.as_slice()).unwrap_or(&[]);
        let value = data.get(0..4).and_then(|b| b.try_into().ok()).map(i32::from_le_bytes);
        let weight = f32_at(data, 4);
        let damage = u16_at(data, 8);
        println!(
            "{edid} 0x{:08x} \"{full}\" DATA({}B) value={value:?} weight={weight:?} damage={damage:?}",
            r.form_id,
            data.len()
        );
        println!("  DATA hex: {}", hex(data));
        if let Some(dnam) = r.field(b"DNAM") {
            println!("  DNAM({}B) hex: {}", dnam.data.len(), hex(&dnam.data));
            // f32 reads at every 4-byte offset, u32 reads too —
            // speed/reach are f32s somewhere in here.
            let mut f32s = vec![];
            let mut off = 0;
            while off + 4 <= dnam.data.len() {
                if let Some(v) = f32_at(&dnam.data, off) {
                    if v.is_finite() && v != 0.0 && v.abs() < 1000.0 {
                        f32s.push(format!("@{off}={v}"));
                    }
                }
                off += 4;
            }
            println!("  DNAM f32s: {}", f32s.join(" "));
            let mut u32s = vec![];
            let mut off = 0;
            while off + 4 <= dnam.data.len() {
                if let Some(v) = u32_at(&dnam.data, off) {
                    if v != 0 && v < 100000 {
                        u32s.push(format!("@{off}={v}"));
                    }
                }
                off += 4;
            }
            println!("  DNAM u32s: {}", u32s.join(" "));
        } else {
            println!("  DNAM: MISSING");
        }
        if let Some(etyp) = r.field(b"ETYP") {
            println!("  ETYP: {}", hex(&etyp.data));
        }
        if let Some(crdt) = r.field(b"CRDT") {
            println!("  CRDT({}B): {}", crdt.data.len(), hex(&crdt.data));
        }
    }

    // ---- B. Inn ACHR refs -> NPC_ bases ----
    println!("\n=== B. INN ACTORS ===");
    let mut classes: Vec<u32> = vec![];
    // Walk all groups for the inn cell's children (any of the
    // three cell-children group types).
    let mut inn_children: Vec<&Record> = vec![];
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        if matches!(
            g.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        ) && g.label_form_id() == Some(INN_CELL)
        {
            inn_children.extend(g.records());
        }
        stack.extend(g.subgroups());
    }
    let mut inn_refs: Vec<&Record> = vec![];
    for record in &inn_children {
        if record.type_str() == "ACHR" {
            inn_refs.push(record);
        }
    }
    println!("ACHR refs in inn cell: {}", inn_refs.len());
    for achr in &inn_refs {
        let base = achr
            .field(b"NAME")
            .and_then(|f| u32_at(&f.data, 0))
            .unwrap_or(0);
        let pos = achr.field(b"DATA").map(|f| {
            (
                f32_at(&f.data, 0).unwrap_or(0.0),
                f32_at(&f.data, 4).unwrap_or(0.0),
                f32_at(&f.data, 8).unwrap_or(0.0),
            )
        });
        let npc = by_form_id.get(&base);
        match npc {
            Some(n) => {
                let edid = n.edid().unwrap_or_default();
                let full = field_str(n, b"FULL").unwrap_or_default();
                let acbs = n.field(b"ACBS").map(|f| f.data.as_slice()).unwrap_or(&[]);
                let level = i16_at(acbs, 8);
                let calc_min = u16_at(acbs, 10);
                let calc_max = u16_at(acbs, 12);
                let flags = u32_at(acbs, 0);
                let class = n.field(b"CNAM").and_then(|f| u32_at(&f.data, 0));
                let race = n.field(b"RNAM").and_then(|f| u32_at(&f.data, 0));
                if let Some(c) = class {
                    if !classes.contains(&c) {
                        classes.push(c);
                    }
                }
                println!(
                    "ACHR 0x{:08x} -> NPC_ 0x{:08x} {edid} \"{full}\" pos={pos:?}",
                    achr.form_id, base
                );
                println!(
                    "  ACBS({}B): {} | flags={flags:?} level[i16@8]={level:?} calc_min[u16@10]={calc_min:?} calc_max[u16@12]={calc_max:?}",
                    acbs.len(),
                    hex(acbs)
                );
                println!("  class CNAM={class:?} race RNAM={race:?}");
            }
            None => println!("ACHR 0x{:08x} -> base 0x{:08x} NOT FOUND", achr.form_id, base),
        }
    }

    // ---- C. CLAS records ----
    println!("\n=== C. CLASSES ===");
    for class_id in &classes {
        match by_form_id.get(class_id) {
            Some(c) => {
                let edid = c.edid().unwrap_or_default();
                let full = field_str(c, b"FULL").unwrap_or_default();
                println!("CLAS 0x{class_id:08x} {edid} \"{full}\"");
                for field in &c.fields {
                    let ty = field.type_str();
                    println!("  {ty}({}B): {}", field.data.len(), hex(&field.data));
                }
            }
            None => println!("CLAS 0x{class_id:08x} NOT FOUND"),
        }
    }

    // ---- D. Player starting outfit ----
    println!("\n=== D. PLAYER STARTING OUTFIT ===");
    if let Some(player) = by_form_id.get(&PLAYER_NPC) {
        let doft = player.field(b"DOFT").and_then(|f| u32_at(&f.data, 0));
        println!("player DOFT={doft:?}");
        if let Some(outfit_id) = doft {
            if let Some(otft) = by_form_id.get(&outfit_id) {
                println!(
                    "OTFT 0x{outfit_id:08x} {}",
                    otft.edid().unwrap_or_default()
                );
                for field in &otft.fields {
                    if field.field_type == *b"INAM" {
                        let mut off = 0;
                        while off + 4 <= field.data.len() {
                            let id = u32_at(&field.data, off).unwrap_or(0);
                            let entry = by_form_id.get(&id);
                            match entry {
                                Some(e) => println!(
                                    "  INAM 0x{id:08x} {} {}",
                                    e.type_str(),
                                    e.edid().unwrap_or_default()
                                ),
                                None => println!("  INAM 0x{id:08x} NOT FOUND"),
                            }
                            off += 4;
                        }
                    }
                }
            }
        }
    }

    // ---- E. Loose WEAP refs in the inn ----
    println!("\n=== E. LOOSE WEAPONS IN INN ===");
    let mut count = 0;
    {
        let mut stack: Vec<&Group> = plugin.groups.iter().collect();
        let mut inn_children: Vec<&Record> = vec![];
        while let Some(g) = stack.pop() {
            if matches!(
                g.group_type,
                GroupType::CellChildren
                    | GroupType::CellPersistentChildren
                    | GroupType::CellTemporaryChildren
            ) && g.label_form_id() == Some(INN_CELL)
            {
                inn_children.extend(g.records());
            }
            stack.extend(g.subgroups());
        }
        for record in inn_children {
            if record.type_str() != "REFR" {
                continue;
            }
            let base = record
                .field(b"NAME")
                .and_then(|f| u32_at(&f.data, 0))
                .unwrap_or(0);
            let Some(b) = by_form_id.get(&base) else {
                continue;
            };
            if b.type_str() == "WEAP" {
                let pos = record.field(b"DATA").map(|f| {
                    (
                        f32_at(&f.data, 0).unwrap_or(0.0),
                        f32_at(&f.data, 4).unwrap_or(0.0),
                        f32_at(&f.data, 8).unwrap_or(0.0),
                    )
                });
                println!(
                    "REFR 0x{:08x} -> WEAP 0x{:08x} {} pos={pos:?}",
                    record.form_id,
                    base,
                    b.edid().unwrap_or_default()
                );
                count += 1;
            }
        }
    }
    println!("loose WEAP refs: {count}");

    // ---- F. Health-formula validation ----
    println!("\n=== F. HEALTH FORMULA ===");
    // UESP CLAS formula (validated by tes4skyrim against 935/935
    // vanilla auto-calc NPCs):
    //   Health = racial(50) + ACBS offset[i16@20] + 5*(L-1)
    //            + 10*(L-1)/sum(wH,wM,wS) * wH
    // where wH/wM/wS are CLAS DATA bytes 32-34. DNAM.Health is
    // only a cache — compare it against the formula.
    let check = [
        (0x00013479u32, "Orgnar"),
        (0x0003550bu32, "Embry"),
        (0x00013478u32, "Delphine"),
        (0x000a91a0u32, "EncChicken"),
    ];
    for (npc_id, name) in check {
        let Some(n) = by_form_id.get(&npc_id) else {
            println!("{name} 0x{npc_id:08x}: NOT FOUND");
            continue;
        };
        let acbs = n.field(b"ACBS").map(|f| f.data.as_slice()).unwrap_or(&[]);
        let flags = u32_at(acbs, 0).unwrap_or(0);
        let level_field = i16_at(acbs, 8).unwrap_or(1);
        let calc_min = u16_at(acbs, 10).unwrap_or(0);
        let calc_max = u16_at(acbs, 12).unwrap_or(0);
        let offset = i16_at(acbs, 20).unwrap_or(0);
        let auto_calc = flags & 0x10 != 0;
        let pc_mult = flags & 0x80 != 0;
        // Player level is 1 (no XP system).
        let level = if pc_mult {
            let scaled = 1.0 * (level_field as f32 / 1000.0);
            (scaled as i32).clamp(calc_min as i32, calc_max as i32)
        } else {
            level_field as i32
        };
        let class_id = n.field(b"CNAM").and_then(|f| u32_at(&f.data, 0));
        let (wh, wm, ws) = match class_id.and_then(|c| by_form_id.get(&c)) {
            Some(c) => match c.field(b"DATA") {
                Some(d) => (
                    d.data.get(32).copied().unwrap_or(0) as i32,
                    d.data.get(33).copied().unwrap_or(0) as i32,
                    d.data.get(34).copied().unwrap_or(0) as i32,
                ),
                None => (0, 0, 0),
            },
            None => (0, 0, 0),
        };
        let sum = wh + wm + ws;
        let formula = if auto_calc {
            let class_share = if sum > 0 {
                10 * (level - 1) * wh / sum
            } else {
                0
            };
            50 + offset as i32 + 5 * (level - 1) + class_share
        } else {
            50 + offset as i32
        };
        let dnam_health = n
            .field(b"DNAM")
            .and_then(|f| u16_at(&f.data, 0));
        let tplt = n.field(b"TPLT").and_then(|f| u32_at(&f.data, 0));
        println!(
            "{name} 0x{npc_id:08x}: level={level} (field={level_field}, pc_mult={pc_mult}, min={calc_min}, max={calc_max}) auto_calc={auto_calc} offset={offset} class={class_id:?} wH/wM/wS={wh}/{wm}/{ws} formula_health={formula} DNAM.Health(cache)={dnam_health:?} TPLT={tplt:?}"
        );
    }

    // ---- G. ArmorIronSetAll LVLI contents ----
    println!("\n=== G. LVLI ArmorIronSetAll ===");
    if let Some(lvli) = by_form_id.get(&0x00013739u32) {
        for field in &lvli.fields {
            if field.field_type == *b"LVLO" {
                let id = u32_at(&field.data, 4).unwrap_or(0);
                let entry = by_form_id.get(&id);
                match entry {
                    Some(e) => println!(
                        "  LVLO 0x{id:08x} {} {}",
                        e.type_str(),
                        e.edid().unwrap_or_default()
                    ),
                    None => println!("  LVLO 0x{id:08x} NOT FOUND"),
                }
            }
        }
    }

    // ---- H. Inn ACHR rotations (teleport facing) ----
    println!("\n=== H. INN ACHR ROTATIONS ===");
    for achr in &inn_refs {
        let rot = achr.field(b"DATA").map(|f| {
            (
                f32_at(&f.data, 12).unwrap_or(0.0),
                f32_at(&f.data, 16).unwrap_or(0.0),
                f32_at(&f.data, 20).unwrap_or(0.0),
            )
        });
        println!("ACHR 0x{:08x} rot (rx,ry,rz)={rot:?}", achr.form_id);
    }

    // ---- I. RACE starting health ----
    println!("\n=== I. RACE DATA ===");
    for (race_id, name) in [
        (0x00013746u32, "NordRace"),
        (0x00013741u32, "BretonRace"),
        (0x000a919du32, "ChickenRace"),
    ] {
        match by_form_id.get(&race_id) {
            Some(r) => {
                println!("RACE 0x{race_id:08x} {name}");
                for field in &r.fields {
                    if field.field_type == *b"DATA" {
                        println!("  DATA({}B): {}", field.data.len(), hex(&field.data));
                        // f32 reads at every 4-byte offset
                        let mut off = 0;
                        let mut found = vec![];
                        while off + 4 <= field.data.len() {
                            if let Some(v) = f32_at(&field.data, off) {
                                if v.is_finite() && v != 0.0 && v.abs() < 10000.0 {
                                    found.push(format!("@{off}={v}"));
                                }
                            }
                            off += 4;
                        }
                        println!("  DATA f32s: {}", found.join(" "));
                    }
                }
            }
            None => println!("RACE 0x{race_id:08x} {name}: NOT FOUND"),
        }
    }
}
