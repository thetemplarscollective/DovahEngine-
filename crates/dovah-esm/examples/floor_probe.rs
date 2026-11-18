//! Floor investigation (Dylan 02:08: "The floor is ugly"):
//! list every STAT ref in the Sleeping Giant Inn with its base
//! EDID, MODL path, and Z position, sorted by Z — the floor
//! pieces sit at the lowest Z. Also prints each base's MODL
//! so the floor NIF can be checked for UV range / texture.

use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::HashMap;

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

fn base_id(record: &Record) -> Option<u32> {
    record.field(b"NAME").and_then(|f| {
        if f.data.len() == 4 {
            Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
        } else {
            None
        }
    })
}

fn placement_z(record: &Record) -> Option<f32> {
    record.field(b"DATA").and_then(|f| {
        if f.data.len() == 24 {
            Some(f32::from_le_bytes(f.data[8..12].try_into().unwrap()))
        } else {
            None
        }
    })
}

fn model_path(base: &Record) -> String {
    base.field(b"MODL")
        .map(|f| {
            String::from_utf8_lossy(&f.data)
                .trim_end_matches('\0')
                .to_string()
        })
        .unwrap_or_default()
}

fn main() {
    let esm = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("test-data")
        .join("Skyrim.esm");
    let plugin = Plugin::open(&esm).expect("open ESM");
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    let mut by_edid: HashMap<String, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
        if let Some(edid) = record.edid() {
            by_edid.insert(edid, record);
        }
    }
    let inn = by_edid.get("RiverwoodSleepingGiantInn").expect("inn CELL");
    let children = cell_children(&plugin, inn.form_id);

    let mut stats: Vec<(f32, String, String, u32)> = Vec::new();
    for r in &children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(bid) = base_id(r) else { continue };
        let Some(base) = by_form_id.get(&bid) else {
            continue;
        };
        if base.type_str() != "STAT" {
            continue;
        }
        let z = placement_z(r).unwrap_or(f32::NAN);
        let edid = base.edid().unwrap_or_default();
        stats.push((z, edid, model_path(base), r.form_id));
    }
    stats.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    println!("inn STAT refs: {}", stats.len());
    for (z, edid, modl, form) in &stats {
        println!("z={:8.1} 0x{:08x} {:40} {}", z, form, edid, modl);
    }
}
