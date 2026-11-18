//! Blown-out-door probe (v0.4.4 phone frame f002): how close is
//! each inn light to the inn door panel? Bevy's point lights
//! use physical 1/d^2 falloff with no minimum distance, so a
//! light ~0.5 m from the door blows it white at the M14
//! calibration (8000 x range^2 lumens).
//!
//! Usage: cargo run -p dovah-esm --example door_light_probe

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

fn light_radius(base: &Record) -> Option<u32> {
    base.field(b"DATA").and_then(|f| {
        if f.data.len() >= 8 {
            Some(u32::from_le_bytes(f.data[4..8].try_into().unwrap()))
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
    let children = cell_children(&plugin, 0x000133c6);
    // Every DOOR ref in the inn + every light; report each
    // door's nearest light (the blown-out door in the phone
    // frame is a room door, not the load door).
    // (ref form id, base EDID, position, has XTEL)
    type DoorRow = (u32, String, (f32, f32, f32), bool);
    let mut doors: Vec<DoorRow> = Vec::new();
    let mut lights: Vec<(String, (f32, f32, f32), u32)> = Vec::new();
    for r in &children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(base_id) = base_id(r) else { continue };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        let Some(pos) = placement(r) else { continue };
        if base.type_str() == "DOOR" {
            doors.push((
                r.form_id,
                base.edid().unwrap_or_default(),
                pos,
                r.field(b"XTEL").is_some(),
            ));
        }
        if base.type_str() == "LIGH" {
            lights.push((
                base.edid().unwrap_or_default(),
                pos,
                light_radius(base).unwrap_or(0),
            ));
        }
    }
    for (ref_id, edid, door, has_xtel) in doors {
        // Nearest light to this door.
        let mut best: Option<(String, f32, f32)> = None;
        for (ledid, pos, radius_u) in &lights {
            let dx = (pos.0 - door.0) / 70.0;
            let dy = (pos.1 - door.1) / 70.0;
            let dz = (pos.2 - door.2) / 70.0;
            let d = (dx * dx + dy * dy + dz * dz).sqrt();
            let range_m = *radius_u as f32 / 70.0;
            let intensity = 8000.0 * range_m * range_m;
            let irradiance = intensity / (4.0 * std::f32::consts::PI * d * d);
            if best.as_ref().is_none_or(|(_, bd, _)| d < *bd) {
                best = Some((ledid.clone(), d, irradiance));
            }
        }
        let (ledid, d, irradiance) = best.unwrap();
        println!(
            "DOOR ref 0x{ref_id:08x} {edid} xtel={has_xtel} pos=({:.0},{:.0},{:.0}) nearest light {ledid} d={d:.2} m -> {irradiance:.0} lux",
            door.0, door.1, door.2
        );
    }
}
