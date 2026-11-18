//! Light-falloff probe: dump the FULL 48-byte LIGH DATA for every
//! light base referenced by the Sleeping Giant Inn cell, plus the
//! REFR positions (Bevy coords). Identifies the Fade field and
//! Near Clip by value, so the falloff formula can be built from
//! the game's own data instead of guessed.
//!
//! Usage: cargo run -p dovah-app --example ligh_data_dump

use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const INN_FORM_ID: u32 = 0x000133c6;

fn f32_at(data: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

fn u32_at(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    // Inn cell children.
    let mut children: Vec<&Record> = Vec::new();
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        if matches!(
            g.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        ) && g.label_form_id() == Some(INN_FORM_ID)
        {
            children.extend(g.records());
        }
        stack.extend(g.subgroups());
    }
    println!("inn children: {}", children.len());
    let mut seen_bases: HashMap<u32, ()> = HashMap::new();
    for r in children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(name_f) = r.field(b"NAME") else {
            continue;
        };
        if name_f.data.len() != 4 {
            continue;
        }
        let base_id = u32::from_le_bytes(name_f.data[0..4].try_into().unwrap());
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        if base.type_str() != "LIGH" {
            continue;
        }
        let pos = r
            .field(b"DATA")
            .map(|f| (f32_at(&f.data, 0), f32_at(&f.data, 4), f32_at(&f.data, 8)));
        // Bevy = (x, z, -y) / 70.
        let bevy = pos.map(|(x, y, z)| (x / 70.0, z / 70.0, -y / 70.0));
        println!(
            "REFR 0x{:08x} base {} (0x{:08x}) skyrim {:?} bevy {:?}",
            r.form_id,
            base.edid().unwrap_or_default(),
            base_id,
            pos,
            bevy
        );
        if seen_bases.insert(base_id, ()).is_some() {
            continue;
        }
        let Some(d) = base.field(b"DATA").map(|f| f.data.as_slice()) else {
            println!("  (no DATA field)");
            continue;
        };
        // Other fields on the LIGH base (FNAM = fade value?).
        let mut others: Vec<String> = Vec::new();
        for f in base.fields(b"FNAM") {
            if f.data.len() == 4 {
                others.push(format!(
                    "FNAM f32={:.4} u32={}",
                    f32_at(&f.data, 0),
                    u32_at(&f.data, 0)
                ));
            }
        }
        if !others.is_empty() {
            println!("  other fields: {}", others.join(", "));
        }
        println!("  DATA len {} bytes:", d.len());
        for slot in 0..(d.len() / 4) {
            let off = slot * 4;
            println!(
                "    [{:2}] u32={:>10} 0x{:08x}  f32={:>10.4}",
                off,
                u32_at(d, off),
                u32_at(d, off),
                f32_at(d, off)
            );
        }
    }
}
