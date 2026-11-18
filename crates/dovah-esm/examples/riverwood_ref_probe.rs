//! Gameplay-evidence probe (v0.4.4 phone frames): what are the
//! yellow placeholder boxes outdoors, and which outdoor actor is
//! naked? Lists every REFR/ACHR in the Riverwood 3x3 exterior
//! block (grid x 4..6, y -12..-10) plus the persistent cell
//! (0x00000d74) whose base is a LIGH, an actor, or model-less.
//!
//! Usage: cargo run -p dovah-esm --example riverwood_ref_probe

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

fn model_path(base: &Record) -> Option<String> {
    base.field(b"MODL").map(|f| {
        String::from_utf8_lossy(&f.data)
            .trim_end_matches('\0')
            .to_string()
    })
}

fn obnd(base: &Record) -> Option<([i16; 3], [i16; 3])> {
    base.field(b"OBND").and_then(|f| {
        if f.data.len() == 12 {
            let min = [
                i16::from_le_bytes(f.data[0..2].try_into().unwrap()),
                i16::from_le_bytes(f.data[2..4].try_into().unwrap()),
                i16::from_le_bytes(f.data[4..6].try_into().unwrap()),
            ];
            let max = [
                i16::from_le_bytes(f.data[6..8].try_into().unwrap()),
                i16::from_le_bytes(f.data[8..10].try_into().unwrap()),
                i16::from_le_bytes(f.data[10..12].try_into().unwrap()),
            ];
            Some((min, max))
        } else {
            None
        }
    })
}

fn form_field(record: &Record, code: &[u8; 4]) -> Option<u32> {
    record.field(code).and_then(|f| {
        if f.data.len() == 4 {
            Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
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
    // The 3x3 block around the inn door's XTEL cell (5,-11),
    // from TAMRIEL's WorldChildren group only — XCLC grid
    // numbers alone are ambiguous across worldspaces (Sovngarde
    // reuses the same grid).
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
    cells.sort();
    println!("block cells: {cells:?}");
    let mut refs: Vec<&Record> = Vec::new();
    for (_, _, id) in &cells {
        refs.extend(cell_children(&plugin, *id));
    }
    refs.extend(cell_children(&plugin, 0x00000d74));
    println!("total refs in block + persistent: {}", refs.len());

    let mut ligh = 0usize;
    let mut ligh_with_model = 0usize;
    let mut model_less = 0usize;
    let mut actors = 0usize;
    for r in &refs {
        let kind = r.type_str();
        if kind != "REFR" && kind != "ACHR" {
            continue;
        }
        let Some(base_id) = base_id(r) else { continue };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        let base_type = base.type_str();
        let edid = base.edid().unwrap_or_default();
        let model = model_path(base);
        let pos = placement(r).unwrap_or((0.0, 0.0, 0.0));
        if base_type == "LIGH" {
            ligh += 1;
            if model.is_some() {
                ligh_with_model += 1;
            }
            println!(
                "LIGH ref 0x{:08x} base {edid} model={:?} obnd={:?} pos=({:.0},{:.0},{:.0})",
                r.form_id,
                model,
                obnd(base),
                pos.0,
                pos.1,
                pos.2
            );
        } else if kind == "ACHR" {
            actors += 1;
            let race = form_field(base, b"RNAM")
                .and_then(|id| by_form_id.get(&id))
                .and_then(|r| r.edid())
                .unwrap_or_default();
            let outfit = form_field(base, b"DOFT")
                .and_then(|id| by_form_id.get(&id))
                .and_then(|r| r.edid())
                .unwrap_or_default();
            let female = base
                .field(b"ACBS")
                .map(|f| f.data.len() > 10 && f.data[10] & 1 != 0)
                .unwrap_or(false);
            let xesp = r.field(b"XESP").map(|f| {
                if f.data.len() >= 5 {
                    format!(
                        "parent=0x{:08x} flags=0x{:02x}",
                        u32::from_le_bytes(f.data[0..4].try_into().unwrap()),
                        f.data[4]
                    )
                } else {
                    format!("len={}", f.data.len())
                }
            });
            println!(
                "ACHR ref 0x{:08x} npc {edid} race={race} female={female} outfit={outfit} flags=0x{:08x} xesp={xesp:?} pos=({:.0},{:.0},{:.0})",
                r.form_id, r.flags, pos.0, pos.1, pos.2
            );
        } else if model.is_none() {
            model_less += 1;
            println!("MODEL-LESS ref 0x{:08x} base {base_type} {edid}", r.form_id);
        }
    }
    println!("---");
    println!("LIGH refs: {ligh} (with MODL: {ligh_with_model})");
    println!("actors: {actors}");
    println!("model-less non-LIGH non-actor refs: {model_less}");
}
