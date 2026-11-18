//! One-off probe: list every ref in the Riverwood 3x3 block
//! (plus the persistent cell) whose base model lives under
//! `meshes/effects/`, with positions — to identify the big
//! grey sheet in the exterior render (too wide to be newFog,
//! unaffected by the gradient multiply).
//!
//! Usage: cargo run -q -p dovah-esm --example effects_probe

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

fn placement(record: &Record) -> Option<(f32, f32, f32)> {
    record.field(b"DATA").and_then(|f| {
        if f.data.len() == 24 {
            Some((
                f32::from_le_bytes(f.data[0..4].try_into().unwrap()),
                f32::from_le_bytes(f.data[4..8].try_into().unwrap()),
                f32::from_le_bytes(f.data[8..12].try_into().unwrap()),
            ))
        } else {
            None
        }
    })
}

fn main() {
    let esm = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("test-data")
        .join("Skyrim.esm");
    let plugin = Plugin::open(&esm).expect("open ESM");
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    let mut cells: Vec<(i32, i32, u32)> = Vec::new();
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        if g.group_type == GroupType::WorldChildren && g.label_form_id() == Some(0x0000003c) {
            let mut inner: Vec<&Group> = vec![g];
            while let Some(h) = inner.pop() {
                for record in h.records() {
                    if record.type_str() != "CELL" {
                        continue;
                    }
                    let Some(f) = record.field(b"XCLC") else {
                        continue;
                    };
                    if f.data.len() < 8 {
                        continue;
                    }
                    let x = i32::from_le_bytes(f.data[0..4].try_into().unwrap());
                    let y = i32::from_le_bytes(f.data[4..8].try_into().unwrap());
                    if (4..=6).contains(&x) && (-12..=-10).contains(&y) {
                        cells.push((x, y, record.form_id));
                    }
                }
                inner.extend(h.subgroups());
            }
        }
        stack.extend(g.subgroups());
    }
    let mut refs: Vec<&Record> = Vec::new();
    for (_, _, id) in &cells {
        refs.extend(cell_children(&plugin, *id));
    }
    // Persistent cell refs, position-filtered to the block
    // rectangle (x 16384..28672, y -49152..-36864).
    for r in cell_children(&plugin, 0x00000d74) {
        if let Some((x, y, _)) = placement(r) {
            if (16384.0..28672.0).contains(&x) && (-49152.0..-36864.0).contains(&y) {
                refs.push(r);
            }
        }
    }
    println!("{} refs in block + persistent sweep", refs.len());
    let mut n_effects = 0;
    for r in refs {
        let Some(bid) = base_id(r) else { continue };
        let Some(base) = by_form_id.get(&bid) else {
            continue;
        };
        let Some(modl) = base.field(b"MODL") else {
            continue;
        };
        let modl = String::from_utf8_lossy(&modl.data);
        let modl = modl.trim_end_matches('\0');
        if !modl.to_lowercase().contains("effects\\") && !modl.to_lowercase().contains("effects/") {
            continue;
        }
        n_effects += 1;
        let edid = base
            .field(b"EDID")
            .map(|f| {
                String::from_utf8_lossy(&f.data)
                    .trim_end_matches('\0')
                    .to_string()
            })
            .unwrap_or_default();
        println!(
            "ref 0x{:08x} base 0x{:08x} {} MODL={} pos={:?}",
            r.form_id,
            bid,
            edid,
            modl,
            placement(r)
        );
    }
    println!("{n_effects} effects refs");
}
