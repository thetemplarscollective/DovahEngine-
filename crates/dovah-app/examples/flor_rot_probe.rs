//! One-off M17 probe: do the inn's FLOR refs (hanging animals
//! sharing a ring position) have DISTINCT rotations? If yes,
//! (position + rotation) uniquely identifies each one for the
//! pickup despawn match.

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
    // (pos bits, rot bits) -> refs, for FLOR only.
    let mut map: HashMap<[u32; 6], Vec<(u32, String)>> = HashMap::new();
    for r in &children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(bid) = r.field(b"NAME").and_then(|f| {
            if f.data.len() == 4 {
                Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
            } else {
                None
            }
        }) else {
            continue;
        };
        let Some(base) = by_form_id.get(&bid) else {
            continue;
        };
        if base.type_str() != "FLOR" {
            continue;
        }
        let Some(data) = r.field(b"DATA") else {
            println!("FLOR ref 0x{:08x} has NO DATA", r.form_id);
            continue;
        };
        let nums: Vec<f32> = data
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .take(6)
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        let key: [u32; 6] = [
            nums[0].to_bits(),
            nums[1].to_bits(),
            nums[2].to_bits(),
            nums[3].to_bits(),
            nums[4].to_bits(),
            nums[5].to_bits(),
        ];
        map.entry(key)
            .or_default()
            .push((r.form_id, base.edid().unwrap_or_default()));
        println!(
            "  FLOR ref 0x{:08x} {} pos=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3})",
            r.form_id,
            base.edid().unwrap_or_default(),
            nums[0],
            nums[1],
            nums[2],
            nums[3],
            nums[4],
            nums[5]
        );
    }
    println!("\n== (position + rotation) collisions among FLOR ==");
    let mut collisions = 0;
    for (key, refs) in &map {
        if refs.len() > 1 {
            collisions += 1;
            println!("  key {key:?}: {} refs: {refs:?}", refs.len());
        }
    }
    if collisions == 0 {
        println!("  (none — every FLOR ref is unique by position+rotation)");
    }

    // Non-FLOR refs sharing a FLOR cluster position (the rings):
    // their rotations must differ from every animal's, or the
    // rotation-aware despawn would take the ring with a rabbit.
    println!("\n== non-FLOR refs at FLOR positions ==");
    let flor_positions: Vec<[u32; 3]> = map
        .keys()
        .map(|k| [k[0], k[1], k[2]])
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    for r in &children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(bid) = r.field(b"NAME").and_then(|f| {
            if f.data.len() == 4 {
                Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
            } else {
                None
            }
        }) else {
            continue;
        };
        let Some(base) = by_form_id.get(&bid) else {
            continue;
        };
        if base.type_str() == "FLOR" {
            continue;
        }
        let Some(data) = r.field(b"DATA") else {
            continue;
        };
        let nums: Vec<f32> = data
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .take(6)
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        let pos = [nums[0].to_bits(), nums[1].to_bits(), nums[2].to_bits()];
        if flor_positions.contains(&pos) {
            println!(
                "  ref 0x{:08x} {} {} rot=({:.3},{:.3},{:.3})",
                r.form_id,
                base.type_str(),
                base.edid().unwrap_or_default(),
                nums[3],
                nums[4],
                nums[5]
            );
        }
    }
}
