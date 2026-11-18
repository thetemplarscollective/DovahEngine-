//! Diagnose the M9 "0 doors" finding: is Tamriel's persistent
//! cell (form 0x00000d74, XCLC grid (0,0)) colliding in the
//! grid map with the geographic exterior cell at (0,0)?
//! Prints every Tamriel CELL with XCLC (0,0) and how many child
//! records each one has.

use dovah_esm::{GroupType, Plugin};
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const TAMRIEL_WRLD: u32 = 0x0000003c;

fn parse_xclc(data: &[u8]) -> Option<(i32, i32)> {
    if data.len() != 12 {
        return None;
    }
    let gx = i32::from_le_bytes(data[0..4].try_into().unwrap());
    let gy = i32::from_le_bytes(data[4..8].try_into().unwrap());
    Some((gx, gy))
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");

    // Child counts per cell, mirroring CellIndex.
    let mut child_counts: HashMap<u32, usize> = HashMap::new();
    let mut stack: Vec<&dovah_esm::Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        if matches!(
            g.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        ) {
            if let Some(cell_id) = g.label_form_id() {
                *child_counts.entry(cell_id).or_default() += g.records().count();
            }
        }
        stack.extend(g.subgroups());
    }

    // Find the Tamriel WorldChildren group.
    let mut stack: Vec<&dovah_esm::Group> = plugin.groups.iter().collect();
    let mut tamriel_group = None;
    while let Some(g) = stack.pop() {
        if g.group_type == GroupType::WorldChildren && g.label_form_id() == Some(TAMRIEL_WRLD) {
            tamriel_group = Some(g);
            break;
        }
        stack.extend(g.subgroups());
    }
    let tg = tamriel_group.expect("Tamriel WorldChildren group");

    // Every CELL in Tamriel with XCLC (0,0).
    let mut stack2: Vec<&dovah_esm::Group> = vec![tg];
    let mut at_origin = Vec::new();
    while let Some(g) = stack2.pop() {
        for r in g.records() {
            if r.type_str() == "CELL" {
                if let Some(f) = r.field(b"XCLC") {
                    if parse_xclc(&f.data) == Some((0, 0)) {
                        at_origin.push((r.form_id, r.edid().unwrap_or_default()));
                    }
                }
            }
        }
        stack2.extend(g.subgroups());
    }
    println!("Tamriel CELLs with XCLC (0,0): {}", at_origin.len());
    for (form_id, edid) in &at_origin {
        let kids = child_counts.get(form_id).copied().unwrap_or(0);
        println!("  form 0x{form_id:08x} edid={edid:?} children={kids}");
    }
    println!(
        "children of 0x00000d74: {}",
        child_counts.get(&0x00000d74).copied().unwrap_or(0)
    );
}
