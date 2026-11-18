//! VHGT layout sweep: find the Skyrim SE LAND decode that makes
//! neighbouring cells' shared edges line up (the decisive test —
//! real terrain is continuous) AND puts ground objects (FLOR/TREE)
//! and the inn door on the terrain.
//!
//! Layouts: sequential cumsum of int8 deltas (L2), row-reset cumsum,
//! column-major cumsum, absolute int8 * scale, float-first (L1).
//! Orientations: none, flip N-S, flip E-W, transpose.

use dovah_esm::Plugin;
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const CELL_SIZE_UNITS: f32 = 4096.0;

fn parse_placement(data: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    if data.len() != 24 {
        return None;
    }
    let f = |i: usize| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    Some(([f(0), f(1), f(2)], [f(3), f(4), f(5)]))
}

/// Decode VHGT under a layout hypothesis. Returns the 33x33 grid
/// in STORED order (index = row*33+col as laid out in the file).
fn decode(layout: &str, vhgt: &[u8], offset: i32) -> Option<[f32; 33 * 33]> {
    if vhgt.len() != 1096 {
        return None;
    }
    let mut h = [0.0f32; 33 * 33];
    match layout {
        // Sequential cumulative sum of int8 deltas from byte 0.
        "seq" => {
            let mut acc = 0.0f32;
            for i in 0..33 * 33 {
                acc += vhgt[i] as i8 as f32;
                h[i] = offset as f32 + acc;
            }
        }
        // Row-reset: each row starts fresh from the offset; deltas
        // accumulate only within the row.
        "rowreset" => {
            for y in 0..33 {
                let mut acc = 0.0f32;
                for x in 0..33 {
                    acc += vhgt[y * 33 + x] as i8 as f32;
                    h[y * 33 + x] = offset as f32 + acc;
                }
            }
        }
        // Column-major cumulative sum.
        "colmajor" => {
            let mut acc = 0.0f32;
            for x in 0..33 {
                for y in 0..33 {
                    acc += vhgt[x * 33 + y] as i8 as f32;
                    h[y * 33 + x] = offset as f32 + acc;
                }
            }
        }
        // Absolute int8 values * scale (no accumulation).
        "abs1" | "abs2" | "abs4" | "abs8" | "abs16" => {
            let s: f32 = layout[3..].parse().unwrap();
            for i in 0..33 * 33 {
                h[i] = offset as f32 + vhgt[i] as i8 as f32 * s;
            }
        }
        // Float first height + int8 deltas from byte 4.
        "floatfirst" => {
            let first = f32::from_le_bytes(vhgt[0..4].try_into().unwrap());
            let mut acc = first;
            for i in 0..33 * 33 {
                acc += vhgt[4 + i] as i8 as f32;
                h[i] = offset as f32 + acc;
            }
        }
        // Float first + PER-ROW cumsum: each row starts from the
        // first height, deltas accumulate only within the row.
        // The N/S byte match (28/33 exact, 5 explained) proves the
        // deltas are row-major with a 4-byte float header; the
        // row-reset is what lets E/W edges line up, because each
        // row's heights then depend only on that row's deltas.
        "l1rowreset" => {
            let first = f32::from_le_bytes(vhgt[0..4].try_into().unwrap());
            for y in 0..33 {
                let mut acc = first;
                for x in 0..33 {
                    acc += vhgt[4 + y * 33 + x] as i8 as f32;
                    h[y * 33 + x] = offset as f32 + acc;
                }
            }
        }
        // Same but WITHOUT the DATA offset (offset is uniform 31 in
        // this area, so this only shifts everything by 31).
        "l1rowreset_nooff" => {
            let first = f32::from_le_bytes(vhgt[0..4].try_into().unwrap());
            for y in 0..33 {
                let mut acc = first;
                for x in 0..33 {
                    acc += vhgt[4 + y * 33 + x] as i8 as f32;
                    h[y * 33 + x] = acc;
                }
            }
        }
        // Float first + per-COLUMN cumsum.
        "l1colreset" => {
            let first = f32::from_le_bytes(vhgt[0..4].try_into().unwrap());
            for x in 0..33 {
                let mut acc = first;
                for y in 0..33 {
                    acc += vhgt[4 + y * 33 + x] as i8 as f32;
                    h[y * 33 + x] = offset as f32 + acc;
                }
            }
        }
        // MIXED: first column accumulates down (each row's first
        // vertex = previous row's first vertex + its delta); each
        // row then accumulates across from its first vertex.
        // (Sequential cumsum instead continues from the END of the
        // previous row — the difference is one row's total rise.)
        "l1mixed" => {
            let first = f32::from_le_bytes(vhgt[0..4].try_into().unwrap());
            let d = |i: usize| vhgt[4 + i] as i8 as f32;
            let mut col_acc = first;
            for y in 0..33 {
                col_acc += d(y * 33);
                let mut row_acc = col_acc;
                // h[y][0] = col_acc; deltas from x=1 on.
                h[y * 33] = offset as f32 + row_acc;
                for x in 1..33 {
                    row_acc += d(y * 33 + x);
                    h[y * 33 + x] = offset as f32 + row_acc;
                }
            }
        }
        // Same mixed accumulation, no float header (deltas from
        // byte 0, base = offset only).
        "l2mixed" => {
            let d = |i: usize| vhgt[i] as i8 as f32;
            let mut col_acc = 0.0f32;
            for y in 0..33 {
                col_acc += d(y * 33);
                let mut row_acc = col_acc;
                h[y * 33] = offset as f32 + row_acc;
                for x in 1..33 {
                    row_acc += d(y * 33 + x);
                    h[y * 33 + x] = offset as f32 + row_acc;
                }
            }
        }
        _ => return None,
    }
    Some(h)
}

/// Oriented grid access: x = 0..32 west->east, y = 0..32 south->north.
fn get(
    h: &[f32; 33 * 33],
    x: usize,
    y: usize,
    flip_ns: bool,
    flip_ew: bool,
    transpose: bool,
) -> f32 {
    let (mut sx, mut sy) = (x, y);
    if transpose {
        std::mem::swap(&mut sx, &mut sy);
    }
    if flip_ew {
        sx = 32 - sx;
    }
    if flip_ns {
        sy = 32 - sy;
    }
    h[sy * 33 + sx]
}

// Probe helper: 8 params is the sampling coordinate bundle;
// splitting it into a struct would only obscure the sweep.
#[allow(clippy::too_many_arguments)]
fn sample(
    h: &[f32; 33 * 33],
    gx: i32,
    gy: i32,
    x: f32,
    y: f32,
    flip_ns: bool,
    flip_ew: bool,
    transpose: bool,
) -> Option<f32> {
    let fx = (x - gx as f32 * CELL_SIZE_UNITS) / 128.0;
    let fy = (y - gy as f32 * CELL_SIZE_UNITS) / 128.0;
    if !(0.0..=32.0).contains(&fx) || !(0.0..=32.0).contains(&fy) {
        return None;
    }
    let x0 = (fx.floor() as usize).min(31);
    let y0 = (fy.floor() as usize).min(31);
    let txf = fx - x0 as f32;
    let tyf = fy - y0 as f32;
    let h00 = get(h, x0, y0, flip_ns, flip_ew, transpose);
    let h10 = get(h, x0 + 1, y0, flip_ns, flip_ew, transpose);
    let h01 = get(h, x0, y0 + 1, flip_ns, flip_ew, transpose);
    let h11 = get(h, x0 + 1, y0 + 1, flip_ns, flip_ew, transpose);
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

    // Raw VHGT + offset per cell in the Riverwood area.
    let mut raw: HashMap<(i32, i32), (Vec<u8>, i32)> = HashMap::new();
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
            raw.insert((gx, gy), (vhgt.data.clone(), offset));
        }
    }

    // Ground-truth objects in the target cell (5,-11): FLOR + TREE
    // bases sit on the terrain; plus the inn door point.
    let target = cells.get(&(5, -11)).unwrap();
    let trecs = children.get(&target.form_id).unwrap();
    let mut ground_pts: Vec<(f32, f32, f32)> = Vec::new();
    for r in trecs {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) else {
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
            .unwrap_or_default();
        if btype == "FLOR" || btype == "TREE" {
            ground_pts.push((pos[0], pos[1], pos[2]));
        }
    }
    println!(
        "ground-truth points (FLOR+TREE) in (5,-11): {}",
        ground_pts.len()
    );

    let layouts = [
        "seq",
        "rowreset",
        "colmajor",
        "abs1",
        "abs2",
        "abs4",
        "abs8",
        "abs16",
        "floatfirst",
        "l1rowreset",
        "l1rowreset_nooff",
        "l1colreset",
        "l1mixed",
        "l2mixed",
    ];
    let mut results: Vec<(f32, String)> = Vec::new();
    for layout in layouts {
        // Decode all cells under this layout.
        let mut lands: HashMap<(i32, i32), [f32; 33 * 33]> = HashMap::new();
        for (k, (vhgt, offset)) in &raw {
            if let Some(h) = decode(layout, vhgt, *offset) {
                lands.insert(*k, h);
            }
        }
        for flip_ns in [false, true] {
            for flip_ew in [false, true] {
                for transpose in [false, true] {
                    // Edge continuity: worst |diff| over all shared
                    // edges, split by direction (a layout can nail
                    // one direction and fail the other).
                    let mut worst = 0.0f32;
                    let mut worst_ew = 0.0f32;
                    let mut worst_ns = 0.0f32;
                    for (&(gx, gy), h) in &lands {
                        if let Some(e) = lands.get(&(gx + 1, gy)) {
                            for y in 0..33 {
                                let d = (get(h, 32, y, flip_ns, flip_ew, transpose)
                                    - get(e, 0, y, flip_ns, flip_ew, transpose))
                                .abs();
                                worst = worst.max(d);
                                worst_ew = worst_ew.max(d);
                            }
                        }
                        if let Some(n) = lands.get(&(gx, gy + 1)) {
                            for x in 0..33 {
                                let d = (get(h, x, 32, flip_ns, flip_ew, transpose)
                                    - get(n, x, 0, flip_ns, flip_ew, transpose))
                                .abs();
                                worst = worst.max(d);
                                worst_ns = worst_ns.max(d);
                            }
                        }
                    }
                    // Ground-object fit in the target cell.
                    let h = lands.get(&(5, -11)).unwrap();
                    let mut diffs: Vec<f32> = Vec::new();
                    for (x, y, z) in &ground_pts {
                        if let Some(th) = sample(h, 5, -11, *x, *y, flip_ns, flip_ew, transpose) {
                            diffs.push(z - th);
                        }
                    }
                    diffs.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let med = diffs[diffs.len() / 2];
                    let door = sample(h, 5, -11, 23259.8, -44476.2, flip_ns, flip_ew, transpose)
                        .map(|th| -8.3 - th)
                        .unwrap_or(f32::NAN);
                    results.push((
                        worst,
                        format!(
                            "{layout:10} flip_ns={flip_ns} flip_ew={flip_ew} transpose={transpose}: worst {worst:8.1} (E/W {worst_ew:8.1}, N/S {worst_ns:8.1}), ground median {med:8.1}, door {door:8.1}"
                        ),
                    ));
                }
            }
        }
    }
    results.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    println!("\nVHGT sweep — ranked by worst edge mismatch (best 12 of 112):");
    for (_, line) in results.iter().take(12) {
        println!("  {line}");
    }
}
