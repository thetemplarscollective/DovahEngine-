//! Archery probe (archery milestone): what does the real data say
//! about arrows, bows, and where the player can get them?
//!
//! A. AMMO records by EDID: FULL, DATA bytes (value/weight/damage),
//!    DNAM bytes (full hex + f32/u32 reads — projectile speed
//!    lives here somewhere, if anywhere), MODL (the arrow model),
//!    and every field type + length.
//! B. Bow WEAP records by EDID: FULL, DATA, DNAM f32s (speed/reach
//!    already known from the melee probe — re-confirm for bows),
//!    ETYP (animation type), MODL.
//! C. Loose AMMO + bow refs in the inn (CELL 0x000133c6) and the
//!    Riverwood exterior 3x3 (centre (5,-11), found by XCLC).
//! D. Player starting outfit: any AMMO or bow in it?
//!
//! Usage: cargo run -p dovah-esm --example archery_probe
//! (run from the workspace root; reads test-data/)

use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::HashMap;
use std::path::Path;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const INN_CELL: u32 = 0x000133c6;
const PLAYER_NPC: u32 = 0x00000007;

const ARROWS: &[&str] = &[
    "IronArrow",
    "SteelArrow",
    "OrcishArrow",
    "DwarvenArrow",
    "ElvenArrow",
    "GlassArrow",
    "EbonyArrow",
    "DaedricArrow",
    "AncientNordArrow",
    "FalmerArrow",
    "ForswornArrow",
    "NordHeroArrow",
    "WoodenArrow",
    "HuntingArrow",
];

const BOWS: &[&str] = &[
    "HuntingBow",
    "LongBow",
    "ImperialBow",
    "OrcishBow",
    "DwarvenBow",
    "ElvenBow",
    "GlassBow",
    "EbonyBow",
    "DaedricBow",
    "AncientNordBow",
    "ForswornBow",
    "FalmerBow",
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

fn field_str(record: &Record, ty: &[u8; 4]) -> Option<String> {
    record.field(ty).map(|f| {
        String::from_utf8_lossy(&f.data)
            .trim_end_matches('\0')
            .to_string()
    })
}

fn dump_f32s(data: &[u8]) -> String {
    let mut out = vec![];
    let mut off = 0;
    while off + 4 <= data.len() {
        if let Some(v) = f32_at(data, off) {
            if v.is_finite() && v != 0.0 && v.abs() < 100000.0 {
                out.push(format!("@{off}={v}"));
            }
        }
        off += 4;
    }
    out.join(" ")
}

fn cell_children<'a>(plugin: &'a Plugin, cell_id: u32) -> Vec<&'a Record> {
    let mut children = vec![];
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        if matches!(
            g.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        ) && g.label_form_id() == Some(cell_id)
        {
            children.extend(g.records());
        }
        stack.extend(g.subgroups());
    }
    children
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

    // ---- A. AMMO records ----
    println!("=== A. AMMO ===");
    for edid in ARROWS {
        let Some(r) = by_edid.get(*edid) else {
            println!("{edid}: NOT FOUND");
            continue;
        };
        if r.type_str() != "AMMO" {
            println!("{edid}: type {} (not AMMO)", r.type_str());
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
        for field in &r.fields {
            let ty = field.type_str();
            if ty == "DATA" || ty == "EDID" || ty == "FULL" {
                continue;
            }
            println!("  {ty}({}B): {}", field.data.len(), hex(&field.data));
            if field.data.len() >= 4 {
                let f = dump_f32s(&field.data);
                if !f.is_empty() {
                    println!("    f32s: {f}");
                }
            }
        }
    }

    // ---- B. Bow WEAP records ----
    println!("\n=== B. BOWS ===");
    for edid in BOWS {
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
        let dnam = r.field(b"DNAM").map(|f| f.data.as_slice()).unwrap_or(&[]);
        println!(
            "{edid} 0x{:08x} \"{full}\" DATA({}B) value={value:?} weight={weight:?} damage={damage:?} DNAM({}B) f32s: {}",
            r.form_id,
            data.len(),
            dnam.len(),
            dump_f32s(dnam)
        );
        if let Some(etyp) = r.field(b"ETYP") {
            println!("  ETYP: {} (u32={:?})", hex(&etyp.data), u32_at(&etyp.data, 0));
        }
        if let Some(modl) = field_str(r, b"MODL") {
            println!("  MODL: {modl}");
        }
        if let Some(crdt) = r.field(b"CRDT") {
            println!("  CRDT({}B): {}", crdt.data.len(), hex(&crdt.data));
        }
    }

    // ---- C. Loose AMMO + bows in the inn and Riverwood exterior ----
    println!("\n=== C. LOOSE ARROWS + BOWS ===");
    // Find the Riverwood 3x3 exterior cells by XCLC grid coords.
    let mut ext_cells: Vec<(i32, i32, u32)> = vec![];
    for record in plugin.all_records() {
        if record.type_str() != "CELL" {
            continue;
        }
        let Some(xclc) = record.field(b"XCLC") else {
            continue;
        };
        let gx = xclc.data.get(0..4).and_then(|b| b.try_into().ok()).map(i32::from_le_bytes);
        let gy = xclc.data.get(4..8).and_then(|b| b.try_into().ok()).map(i32::from_le_bytes);
        if let (Some(x), Some(y)) = (gx, gy) {
            if (4..=6).contains(&x) && (-12..=-10).contains(&y) {
                ext_cells.push((x, y, record.form_id));
            }
        }
    }
    ext_cells.sort();
    println!("Riverwood 3x3 cells: {ext_cells:?}");
    let mut cells: Vec<(String, u32)> = vec![("inn".to_string(), INN_CELL)];
    for (x, y, id) in &ext_cells {
        cells.push((format!("ext ({x},{y})"), *id));
    }
    for (label, cell_id) in &cells {
        let children = cell_children(&plugin, *cell_id);
        let mut found = 0;
        for record in &children {
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
            let is_ammo = b.type_str() == "AMMO";
            // List every loose WEAP (bows identified by EDID in
            // the output — ETYP values are not assumed here).
            let is_weap = b.type_str() == "WEAP";
            if !is_ammo && !is_weap {
                continue;
            }
            let pos = record.field(b"DATA").map(|f| {
                (
                    f32_at(&f.data, 0).unwrap_or(0.0),
                    f32_at(&f.data, 4).unwrap_or(0.0),
                    f32_at(&f.data, 8).unwrap_or(0.0),
                )
            });
            let count = record
                .field(b"CNAM")
                .and_then(|f| u32_at(&f.data, 0))
                .unwrap_or(1);
            println!(
                "{label} REFR 0x{:08x} -> {} 0x{:08x} {} x{count} pos={pos:?}",
                record.form_id,
                b.type_str(),
                base,
                b.edid().unwrap_or_default()
            );
            found += 1;
        }
        println!("{label}: {found} arrow/bow refs");
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
                                Some(e) => {
                                    // LVLI: expand one level for AMMO/WEAP.
                                    if e.type_str() == "LVLI" {
                                        println!("  INAM 0x{id:08x} LVLI {}", e.edid().unwrap_or_default());
                                        for lf in &e.fields {
                                            if lf.field_type == *b"LVLO" {
                                                let lid = u32_at(&lf.data, 4).unwrap_or(0);
                                                match by_form_id.get(&lid) {
                                                    Some(le) => println!(
                                                        "    LVLO 0x{lid:08x} {} {}",
                                                        le.type_str(),
                                                        le.edid().unwrap_or_default()
                                                    ),
                                                    None => println!("    LVLO 0x{lid:08x} NOT FOUND"),
                                                }
                                            }
                                        }
                                    } else {
                                        println!(
                                            "  INAM 0x{id:08x} {} {}",
                                            e.type_str(),
                                            e.edid().unwrap_or_default()
                                        );
                                    }
                                }
                                None => println!("  INAM 0x{id:08x} NOT FOUND"),
                            }
                            off += 4;
                        }
                    }
                }
            }
        }
    }
}
