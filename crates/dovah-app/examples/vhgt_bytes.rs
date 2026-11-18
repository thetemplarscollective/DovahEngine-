//! Raw VHGT byte inspection: compare the shared edge bytes of two
//! adjacent cells directly, and dump the 7 trailing bytes of VHGT
//! (1089..1096) for several cells to see if they carry a per-cell
//! base height.

use dovah_esm::Plugin;
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");

    // Tamriel WorldChildren group walk.
    let mut stack: Vec<&dovah_esm::Group> = plugin.groups.iter().collect();
    let mut tamriel_group: Option<&dovah_esm::Group> = None;
    while let Some(g) = stack.pop() {
        if g.group_type == dovah_esm::GroupType::WorldChildren
            && g.label_form_id() == Some(0x0000003c)
        {
            tamriel_group = Some(g);
            break;
        }
        stack.extend(g.subgroups());
    }
    let tg = tamriel_group.expect("Tamriel WorldChildren group");
    let mut cells: HashMap<(i32, i32), &dovah_esm::Record> = HashMap::new();
    let mut children: HashMap<u32, Vec<&dovah_esm::Record>> = HashMap::new();
    let mut stack2: Vec<&dovah_esm::Group> = vec![tg];
    while let Some(g) = stack2.pop() {
        for r in g.records() {
            if r.type_str() == "CELL" {
                if let Some(f) = r.field(b"XCLC") {
                    if f.data.len() == 12 {
                        let gx = i32::from_le_bytes(f.data[0..4].try_into().unwrap());
                        let gy = i32::from_le_bytes(f.data[4..8].try_into().unwrap());
                        cells.insert((gx, gy), r);
                    }
                }
            }
        }
        if matches!(
            g.group_type,
            dovah_esm::GroupType::CellChildren
                | dovah_esm::GroupType::CellPersistentChildren
                | dovah_esm::GroupType::CellTemporaryChildren
        ) {
            if let Some(cell_id) = g.label_form_id() {
                let entry = children.entry(cell_id).or_default();
                entry.extend(g.records());
            }
        }
        stack2.extend(g.subgroups());
    }

    let get_vhgt = |gx: i32, gy: i32| -> Option<Vec<u8>> {
        let cell = cells.get(&(gx, gy))?;
        let recs = children.get(&cell.form_id)?;
        let land = recs.iter().find(|r| r.type_str() == "LAND")?;
        land.field(b"VHGT").map(|f| f.data.clone())
    };

    // Trailing 7 bytes for the Riverwood 3x3.
    println!("=== VHGT trailing bytes [1089..1096] ===");
    for gy in (-12..=-10).rev() {
        for gx in 4..=6 {
            if let Some(v) = get_vhgt(gx, gy) {
                let tail = &v[1089..1096];
                println!(
                    "  ({gx},{gy}): {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}  as i8 {:?}  last4 as f32 {}  as i32 {}",
                    tail[0], tail[1], tail[2], tail[3], tail[4], tail[5], tail[6],
                    tail.iter().map(|b| *b as i8).collect::<Vec<_>>(),
                    f32::from_le_bytes(tail[3..7].try_into().unwrap()),
                    i32::from_le_bytes(tail[3..7].try_into().unwrap()),
                );
            }
        }
    }

    // First 4 bytes as f32 for the same cells (the L1 candidate).
    println!("\n=== VHGT first 4 bytes as f32 ===");
    for gy in (-12..=-10).rev() {
        for gx in 4..=6 {
            if let Some(v) = get_vhgt(gx, gy) {
                println!(
                    "  ({gx},{gy}): f32 {}  i32 {}  bytes {:02x} {:02x} {:02x} {:02x}",
                    f32::from_le_bytes(v[0..4].try_into().unwrap()),
                    i32::from_le_bytes(v[0..4].try_into().unwrap()),
                    v[0],
                    v[1],
                    v[2],
                    v[3]
                );
            }
        }
    }

    // Shared edge bytes: E edge of (5,-11) vs W edge of (6,-11).
    println!("\n=== E edge bytes of (5,-11) vs W edge bytes of (6,-11) ===");
    let a = get_vhgt(5, -11).unwrap();
    let b = get_vhgt(6, -11).unwrap();
    let mut exact = 0;
    let mut diffs: Vec<i32> = Vec::new();
    for row in 0..33 {
        let ea = a[row * 33 + 32] as i8;
        let wb = b[row * 33] as i8;
        let d = ea as i32 - wb as i32;
        diffs.push(d);
        if d == 0 {
            exact += 1;
        }
        println!("  row {row:2}: E(5,-11)={ea:4}  W(6,-11)={wb:4}  diff {d:4}");
    }
    println!("exact matches: {exact}/33");

    // N edge of (5,-11) vs S edge of (5,-10).
    println!("\n=== N edge bytes of (5,-11) vs S edge bytes of (5,-10) ===");
    let c = get_vhgt(5, -10).unwrap();
    let mut exact2 = 0;
    for col in 0..33 {
        let na = a[32 * 33 + col] as i8;
        let sc = c[col] as i8;
        let d = na as i32 - sc as i32;
        if d == 0 {
            exact2 += 1;
        }
        println!("  col {col:2}: N(5,-11)={na:4}  S(5,-10)={sc:4}  diff {d:4}");
    }
    println!("exact matches: {exact2}/33");
}
