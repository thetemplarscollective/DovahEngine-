//! Secret-door probe: what is around the secret door (0x00093bcd)
//! in the inn basement? Lists every ref within 1500 Skyrim units
//! of the door (-634, 800, 0) so the screenshot camera can be
//! placed on open floor, on the room side, looking at the door.
//!
//! Usage: cargo run -p dovah-app --example basement_probe

use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const INN_EDID: &str = "RiverwoodSleepingGiantInn";
const DOOR_POS: [f32; 3] = [-634.0, 800.0, 0.0];
const RADIUS: f32 = 1500.0;

const FLAG_INITIALLY_DISABLED: u32 = 0x00000800;
const FLAG_IS_MARKER: u32 = 0x00000020;

fn form_id_from_field(record: &Record, field_type: &[u8; 4]) -> Option<u32> {
    let field = record.field(field_type)?;
    if field.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(field.data[0..4].try_into().unwrap()))
}

fn parse_placement(data: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    if data.len() != 24 {
        return None;
    }
    let f = |i: usize| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    Some(([f(0), f(1), f(2)], [f(3), f(4), f(5)]))
}

struct NearRef {
    dist: f32,
    form_id: u32,
    base_type: String,
    edid: String,
    position: [f32; 3],
    rotation: [f32; 3],
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
        ) {
            // Find the inn cell id first.
            children.extend(g.records());
        }
        stack.extend(g.subgroups());
    }
    let inn = plugin
        .all_records()
        .into_iter()
        .find(|r| r.type_str() == "CELL" && r.edid().as_deref() == Some(INN_EDID))
        .expect("inn cell");
    // Re-walk, keeping only the inn cell's groups.
    let mut inn_children: Vec<&Record> = Vec::new();
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        if matches!(
            g.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        ) && g.label_form_id() == Some(inn.form_id)
        {
            inn_children.extend(g.records());
        }
        stack.extend(g.subgroups());
    }
    let _ = children;

    let mut near: Vec<NearRef> = Vec::new();
    for r in &inn_children {
        if r.type_str() != "REFR" {
            continue;
        }
        if r.flags & FLAG_INITIALLY_DISABLED != 0 || r.flags & FLAG_IS_MARKER != 0 {
            continue;
        }
        let Some((position, rotation)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data))
        else {
            continue;
        };
        let dx = position[0] - DOOR_POS[0];
        let dy = position[1] - DOOR_POS[1];
        let dist = (dx * dx + dy * dy).sqrt();
        if dist > RADIUS {
            continue;
        }
        let Some(base_id) = form_id_from_field(r, b"NAME") else {
            continue;
        };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        near.push(NearRef {
            dist,
            form_id: r.form_id,
            base_type: base.type_str().to_string(),
            edid: base.edid().unwrap_or_default(),
            position,
            rotation,
        });
    }
    near.sort_by(|a, b| a.dist.partial_cmp(&b.dist).unwrap());
    println!("refs within {RADIUS}u of the secret door: {}", near.len());
    println!(
        "door Bevy pos: ({:.2}, {:.2}, {:.2}) — faces Bevy +X, panels swing toward Bevy -X",
        DOOR_POS[0] / 70.0,
        DOOR_POS[2] / 70.0,
        -DOOR_POS[1] / 70.0
    );
    for n in &near {
        println!(
            "  {:6.0}u ref 0x{:08x} {:4} {:38} pos=({:7.1},{:7.1},{:7.1}) rot=({:.2},{:.2},{:.2}) Bevy=({:5.2},{:5.2},{:5.2})",
            n.dist,
            n.form_id,
            n.base_type,
            n.edid,
            n.position[0],
            n.position[1],
            n.position[2],
            n.rotation[0],
            n.rotation[1],
            n.rotation[2],
            n.position[0] / 70.0,
            n.position[2] / 70.0,
            -n.position[1] / 70.0
        );
    }
}
