//! VHGT decode validation for Skyrim SE LAND records.
//!
//! Skyrim SE VHGT is 1096 bytes (not the 4368-byte float grid of
//! older games). Candidate layout L2: 33x33 int8 deltas from byte 0
//! (cumulative sum), + 7 trailing bytes; DATA offset optionally
//! added. This example validates the decode two ways that do not
//! depend on guessing which objects sit on the ground:
//!
//! 1. EDGE CONTINUITY: adjacent cells share a terrain edge — the
//!    east edge of cell (x,y) must equal the west edge of (x+1,y),
//!    and the north edge of (x,y) the south edge of (x,y+1).
//!    A wrong decode (wrong scale, wrong row order, wrong base)
//!    breaks continuity.
//! 2. GROUND-OBJECT Z by base type: TREE and FLOR bases sit on the
//!    terrain by construction; their REFR Z should match the
//!    sampled terrain height within a few units.

use dovah_esm::Plugin;
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const CELL_SIZE_UNITS: f32 = 4096.0;

/// Decode a LAND record's VHGT into 33x33 heights (Skyrim units).
/// Layout L2: int8 deltas from byte 0, cumulative sum, plus the
/// LAND DATA offset (i32, the cell's absolute height datum — each
/// cell's offset differs, which is what makes neighbouring edges
/// line up).
fn decode_vhgt(vhgt: &[u8], offset: i32) -> Option<[f32; 33 * 33]> {
    if vhgt.len() != 1096 {
        return None;
    }
    let mut heights = [0.0f32; 33 * 33];
    let mut acc = 0.0f32;
    for i in 0..33 * 33 {
        acc += vhgt[i] as i8 as f32;
        heights[i] = offset as f32 + acc;
    }
    Some(heights)
}

fn parse_placement(data: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    if data.len() != 24 {
        return None;
    }
    let f = |i: usize| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    Some(([f(0), f(1), f(2)], [f(3), f(4), f(5)]))
}

fn sample(heights: &[f32; 33 * 33], gx: i32, gy: i32, x: f32, y: f32) -> Option<f32> {
    let fx = (x - gx as f32 * CELL_SIZE_UNITS) / 128.0;
    let fy = (y - gy as f32 * CELL_SIZE_UNITS) / 128.0;
    if !(0.0..=32.0).contains(&fx) || !(0.0..=32.0).contains(&fy) {
        return None;
    }
    let x0 = (fx.floor() as usize).min(31);
    let y0 = (fy.floor() as usize).min(31);
    let txf = fx - x0 as f32;
    let tyf = fy - y0 as f32;
    let h00 = heights[y0 * 33 + x0];
    let h10 = heights[y0 * 33 + x0 + 1];
    let h01 = heights[(y0 + 1) * 33 + x0];
    let h11 = heights[(y0 + 1) * 33 + x0 + 1];
    Some(
        h00 * (1.0 - txf) * (1.0 - tyf)
            + h10 * txf * (1.0 - tyf)
            + h01 * (1.0 - txf) * tyf
            + h11 * txf * tyf,
    )
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut by_form_id: HashMap<u32, &dovah_esm::Record> = HashMap::new();
    for r in plugin.all_records() {
        by_form_id.insert(r.form_id, r);
    }

    // TAMRIEL ONLY: XCLC grid coords are ambiguous across
    // worldspaces (Sovngarde etc. reuse the same grid numbers).
    // Walk the Tamriel WorldChildren group (label = WRLD form
    // 0x0000003c) and collect its CELLs + their child records.
    let mut tamriel_cells: HashMap<(i32, i32), &dovah_esm::Record> = HashMap::new();
    let mut children: HashMap<u32, Vec<&dovah_esm::Record>> = HashMap::new();
    let mut stack: Vec<&dovah_esm::Group> = plugin.groups.iter().collect();
    // First find the Tamriel WorldChildren group.
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
    // Walk it: CELL records + cell-child groups.
    let mut stack2: Vec<&dovah_esm::Group> = vec![tg];
    while let Some(g) = stack2.pop() {
        for r in g.records() {
            if r.type_str() == "CELL" {
                if let Some(f) = r.field(b"XCLC") {
                    if f.data.len() == 12 {
                        let gx = i32::from_le_bytes(f.data[0..4].try_into().unwrap());
                        let gy = i32::from_le_bytes(f.data[4..8].try_into().unwrap());
                        tamriel_cells.insert((gx, gy), r);
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
    println!("Tamriel exterior cells with XCLC: {}", tamriel_cells.len());
    let cells = tamriel_cells;

    // Decode LAND for the Riverwood 3x3 + neighbours for edge checks.
    let mut lands: HashMap<(i32, i32), [f32; 33 * 33]> = HashMap::new();
    let mut offsets: HashMap<(i32, i32), i32> = HashMap::new();
    for gx in 3..=7 {
        for gy in -13..=-9 {
            let Some(cell) = cells.get(&(gx, gy)) else {
                continue;
            };
            let Some(recs) = children.get(&cell.form_id) else {
                continue;
            };
            let Some(land) = recs.iter().find(|r| r.type_str() == "LAND") else {
                continue;
            };
            let Some(vhgt) = land.field(b"VHGT") else {
                continue;
            };
            let offset = land
                .field(b"DATA")
                .and_then(|f| f.data.get(0..4))
                .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
                .unwrap_or(0);
            offsets.insert((gx, gy), offset);
            if let Some(h) = decode_vhgt(&vhgt.data, offset) {
                lands.insert((gx, gy), h);
            }
        }
    }
    println!("LAND decoded for {} cells", lands.len());
    println!("DATA offsets: {:?}", {
        let mut v: Vec<_> = offsets.iter().collect();
        v.sort();
        v
    });

    // --- Edge continuity ---
    println!("\n=== edge continuity (L2 decode + DATA offset) ===");
    let mut worst = 0.0f32;
    for (&(gx, gy), h) in &lands {
        // East edge of (gx,gy) vs west edge of (gx+1,gy).
        if let Some(e) = lands.get(&(gx + 1, gy)) {
            let mut mind = f32::INFINITY;
            let mut maxd = f32::NEG_INFINITY;
            for row in 0..33 {
                let d = h[row * 33 + 32] - e[row * 33];
                mind = mind.min(d);
                maxd = maxd.max(d);
            }
            println!(
                "  E edge ({gx},{gy}) vs W edge ({},{gy}): signed diff {mind:.1}..{maxd:.1}",
                gx + 1
            );
            worst = worst.max(maxd.abs()).max(mind.abs());
        }
        // North edge of (gx,gy) vs south edge of (gx,gy+1).
        if let Some(n) = lands.get(&(gx, gy + 1)) {
            let mut mind = f32::INFINITY;
            let mut maxd = f32::NEG_INFINITY;
            for col in 0..33 {
                let d = h[32 * 33 + col] - n[col];
                mind = mind.min(d);
                maxd = maxd.max(d);
            }
            println!(
                "  N edge ({gx},{gy}) vs S edge ({gx},{}): signed diff {mind:.1}..{maxd:.1}",
                gy + 1
            );
            worst = worst.max(maxd.abs()).max(mind.abs());
        }
    }
    println!("worst edge mismatch: {worst:.1} Skyrim units");

    // --- Ground-object Z by base type, target cell (5,-11) ---
    let Some(h) = lands.get(&(5, -11)) else {
        println!("no LAND for (5,-11)");
        return;
    };
    let cell = cells.get(&(5, -11)).unwrap();
    let recs = children.get(&cell.form_id).unwrap();
    let mut by_type: HashMap<String, Vec<f32>> = HashMap::new();
    for r in recs {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) else {
            continue;
        };
        let Some(th) = sample(h, 5, -11, pos[0], pos[1]) else {
            continue;
        };
        let base_id = r
            .field(b"NAME")
            .and_then(|f| f.data.get(0..4))
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .unwrap_or(0);
        let btype = by_form_id
            .get(&base_id)
            .map(|b| b.type_str())
            .unwrap_or("?".to_string());
        by_type.entry(btype).or_default().push(pos[2] - th);
    }
    println!("\n=== object Z minus terrain, cell (5,-11), by base type ===");
    let mut types: Vec<_> = by_type.iter().collect();
    types.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    types.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (t, diffs) in types {
        let mut d = diffs.clone();
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = d[d.len() / 2];
        let p10 = d[d.len() / 10];
        let p90 = d[d.len() * 9 / 10];
        println!(
            "  {t:6} n={:3} median {median:7.1} p10 {p10:7.1} p90 {p90:7.1}",
            d.len()
        );
    }
    // The inn door: ground truth point.
    let door_h = sample(h, 5, -11, 23259.8, -44476.2).unwrap();
    println!(
        "\ninn door XTEL pos terrain height: {door_h:.1} (door Z -8.3, diff {})",
        -8.3 - door_h
    );
}
