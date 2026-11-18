//! M9 probe: find Riverwood from the inn's front-door teleport.
//!
//! 1. Find every DOOR ref in CELL RiverwoodSleepingGiantInn with an
//!    XTEL field; print the teleport destination (door form ID,
//!    position, rotation) and the field length (XTEL layout is
//!    verified from real data, not assumed).
//! 2. Resolve the exterior target: the destination door's parent
//!    cell (by scanning cell-child groups for the door ref) and
//!    the grid cell from the XTEL position (floor(pos / 4096)).
//! 3. Enumerate the 3x3 block of exterior cells around the target:
//!    EDID, form ID, grid coords, ref counts by type, LAND
//!    presence + field sizes.
//! 4. For the target cell's LAND: decode VHGT, print height stats,
//!    and cross-check terrain height against placed-object Z
//!    (objects sit on the ground — a wrong decode shows up as a
//!    systematic offset).
//!
//! Usage: cargo run -p dovah-app --example riverwood_probe

use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::{HashMap, HashSet};

const ESM_PATH: &str = "test-data/Skyrim.esm";
const CELL_EDID: &str = "RiverwoodSleepingGiantInn";
const CELL_SIZE_UNITS: f32 = 4096.0;

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

/// One pass over the group tree: cell form ID -> its child
/// records (persistent + temporary + plain children), and ref
/// form ID -> parent cell form ID. Walking the tree once per cell
/// is O(cells x groups) and far too slow over 17k exterior cells.
struct CellIndex<'a> {
    children: HashMap<u32, Vec<&'a Record>>,
    parent_of_ref: HashMap<u32, u32>,
}

impl<'a> CellIndex<'a> {
    fn build(plugin: &'a Plugin) -> Self {
        let mut index = CellIndex {
            children: HashMap::new(),
            parent_of_ref: HashMap::new(),
        };
        for group in &plugin.groups {
            index.scan(group);
        }
        index
    }

    fn scan(&mut self, group: &'a Group) {
        if matches!(
            group.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        ) {
            if let Some(cell_id) = group.label_form_id() {
                let entry = self.children.entry(cell_id).or_default();
                for r in group.records() {
                    entry.push(r);
                    self.parent_of_ref.insert(r.form_id, cell_id);
                }
            }
        }
        for subgroup in group.subgroups() {
            self.scan(subgroup);
        }
    }

    fn children_of(&self, cell_id: u32) -> &[&'a Record] {
        self.children
            .get(&cell_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    fn parent_cell_of(&self, ref_form_id: u32) -> Option<u32> {
        self.parent_of_ref.get(&ref_form_id).copied()
    }
}

fn is_interior(cell: &Record) -> bool {
    cell.field(b"DATA")
        .and_then(|f| f.data.first())
        .map(|b| b & 0x01 != 0)
        .unwrap_or(false)
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let index = CellIndex::build(&plugin);
    println!(
        "cell index: {} cells with children, {} refs mapped to parent cells",
        index.children.len(),
        index.parent_of_ref.len()
    );
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    // Exterior cells by grid coords. Skyrim.esm exterior CELLs carry
    // NO XCLC field (verified: 0 of 16978 have one) — grid coords
    // come from the GRUP block/sub-block labels, or (robust
    // fallback used here) the median position of the cell's refs:
    // refs are placed inside their own cell, so the median ref
    // position / 4096 is the grid cell. Verified against the XTEL
    // target position below.
    let mut exterior_by_grid: HashMap<(i32, i32), &Record> = HashMap::new();
    let mut exterior_count = 0usize;
    let mut interior_count = 0usize;
    let mut xclc_count = 0usize;
    let mut no_refs = 0usize;
    for record in plugin.all_records() {
        if record.type_str() != "CELL" {
            continue;
        }
        if is_interior(record) {
            interior_count += 1;
            continue;
        }
        exterior_count += 1;
        if let Some(f) = record.field(b"XCLC") {
            xclc_count += 1;
            if xclc_count <= 3 {
                println!(
                    "  XCLC on CELL 0x{:08x}: len {} hex {}",
                    record.form_id,
                    f.data.len(),
                    hex(&f.data)
                );
            }
        }
        let recs = index.children_of(record.form_id);
        let mut xs: Vec<f32> = Vec::new();
        let mut ys: Vec<f32> = Vec::new();
        for r in recs {
            if r.type_str() != "REFR" && r.type_str() != "ACHR" {
                continue;
            }
            if let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) {
                xs.push(pos[0]);
                ys.push(pos[1]);
            }
        }
        if xs.is_empty() {
            no_refs += 1;
            continue;
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let gx = (xs[xs.len() / 2] / CELL_SIZE_UNITS).floor() as i32;
        let gy = (ys[ys.len() / 2] / CELL_SIZE_UNITS).floor() as i32;
        exterior_by_grid.insert((gx, gy), record);
    }
    println!("CELLs: {interior_count} interior, {exterior_count} exterior, {xclc_count} with XCLC, {no_refs} exterior cells with no refs");
    println!(
        "exterior grid map: {} cells placed by median ref position",
        exterior_by_grid.len()
    );

    // Tamriel worldspace.
    let tamriel = plugin
        .all_records()
        .into_iter()
        .find(|r| r.type_str() == "WRLD" && r.edid().as_deref() == Some("Tamriel"));
    match tamriel {
        Some(w) => println!("WRLD Tamriel form 0x{:08x}", w.form_id),
        None => println!("WRLD Tamriel NOT FOUND"),
    }

    // --- 1. Inn doors with XTEL ---
    let inn = plugin
        .all_records()
        .into_iter()
        .find(|r| r.type_str() == "CELL" && r.edid().as_deref() == Some(CELL_EDID))
        .expect("inn cell");
    println!(
        "\n=== Inn cell {} form 0x{:08x} ===",
        inn.edid().unwrap(),
        inn.form_id
    );
    let placed = index.children_of(inn.form_id);
    // (ref id, edid, ref pos, ref rot, dest ref, spawn pos, spawn rot)
    type XtelDoor = (u32, String, [f32; 3], [f32; 3], u32, [f32; 3], [f32; 3]);
    let mut xtel_doors: Vec<XtelDoor> = Vec::new();
    for r in placed {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(xtel) = r.field(b"XTEL") else {
            continue;
        };
        let Some(base_id) = form_id_from_field(r, b"NAME") else {
            continue;
        };
        let base = by_form_id.get(&base_id);
        let base_edid = base
            .map(|b| b.edid().unwrap_or_default())
            .unwrap_or_default();
        let base_type = base.map(|b| b.type_str()).unwrap_or_default();
        let (pos, rot) = r
            .field(b"DATA")
            .and_then(|f| parse_placement(&f.data))
            .unwrap_or(([0.0; 3], [0.0; 3]));
        // XTEL layout from the data: Skyrim SE uses 32 bytes —
        // dest door form ID (4) + pos xyz (12) + rot xyz (12) +
        // trailing u32 (4, 0 in the inn door — purpose unknown,
        // possibly flags). Older games used 28 (no trailing u32).
        let d = &xtel.data;
        println!(
            "\nXTEL ref 0x{:08x} base {base_edid} ({base_type} 0x{base_id:08x}) field len {}",
            r.form_id,
            d.len()
        );
        println!(
            "  ref pos=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3})",
            pos[0], pos[1], pos[2], rot[0], rot[1], rot[2]
        );
        if d.len() == 28 || d.len() == 32 {
            let dest = u32::from_le_bytes(d[0..4].try_into().unwrap());
            let f = |i: usize| f32::from_le_bytes(d[i * 4..i * 4 + 4].try_into().unwrap());
            let dpos = [f(1), f(2), f(3)];
            let drot = [f(4), f(5), f(6)];
            let tail = if d.len() == 32 {
                u32::from_le_bytes(d[28..32].try_into().unwrap())
            } else {
                0
            };
            println!(
                "  dest door ref 0x{dest:08x} pos=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3}) tail=0x{tail:08x}",
                dpos[0], dpos[1], dpos[2], drot[0], drot[1], drot[2]
            );
            xtel_doors.push((r.form_id, base_edid, pos, rot, dest, dpos, drot));
        } else {
            println!(
                "  UNEXPECTED XTEL LENGTH — hex: {}",
                hex(&d[..d.len().min(64)])
            );
        }
    }

    // --- 2. Resolve each XTEL target ---
    let mut exterior_target: Option<(i32, i32)> = None;
    for (ref_id, base_edid, _pos, _rot, dest, dpos, _drot) in &xtel_doors {
        println!("\n--- resolving XTEL from ref 0x{ref_id:08x} ({base_edid}) ---");
        // (a) parent cell of the destination door ref.
        match index.parent_cell_of(*dest) {
            Some(cell_id) => {
                let cell = by_form_id.get(&cell_id);
                let edid = cell
                    .map(|c| c.edid().unwrap_or_default())
                    .unwrap_or_default();
                let grid = exterior_by_grid
                    .iter()
                    .find(|(_, c)| c.form_id == cell_id)
                    .map(|(g, _)| *g);
                let interior = cell.map(|c| is_interior(c)).unwrap_or(true);
                println!(
                    "  dest door 0x{dest:08x} lives in CELL {edid} (0x{cell_id:08x}, interior={interior}, grid={grid:?})"
                );
                if !interior {
                    if let Some(g) = grid {
                        exterior_target = Some(g);
                    }
                }
            }
            None => println!("  dest door 0x{dest:08x} parent cell NOT FOUND"),
        }
        // (b) grid cell from the XTEL position — ground truth for
        // where the player arrives, regardless of which cell the
        // door ref itself is filed under.
        let gx = (dpos[0] / CELL_SIZE_UNITS).floor() as i32;
        let gy = (dpos[1] / CELL_SIZE_UNITS).floor() as i32;
        println!("  XTEL pos grid cell: ({gx}, {gy})");
        if let Some(cell) = exterior_by_grid.get(&(gx, gy)) {
            println!(
                "  grid cell ({gx},{gy}) = CELL {} (0x{:08x})",
                cell.edid().unwrap_or_default(),
                cell.form_id
            );
            exterior_target = Some((gx, gy));
        } else {
            println!("  no exterior CELL at grid ({gx},{gy})");
        }
    }

    let Some((tx, ty)) = exterior_target else {
        println!("\nNO EXTERIOR XTEL TARGET FOUND — cannot locate Riverwood");
        return;
    };
    println!("\n=== Riverwood exterior target grid cell: ({tx}, {ty}) ===");

    // --- 3. 3x3 block census ---
    let mut total_objects = 0usize;
    let mut total_land = 0usize;
    let mut type_counts: HashMap<String, usize> = HashMap::new();
    let mut base_type_counts: HashMap<String, usize> = HashMap::new();
    for dy in -1..=1 {
        for dx in -1..=1 {
            let g = (tx + dx, ty + dy);
            let Some(cell) = exterior_by_grid.get(&g) else {
                println!("  grid {g:?}: NO CELL");
                continue;
            };
            let recs = index.children_of(cell.form_id);
            let mut refs = 0usize;
            let mut land: Option<&Record> = None;
            let mut other: HashMap<String, usize> = HashMap::new();
            for r in recs {
                match r.type_str().as_str() {
                    "REFR" | "ACHR" => {
                        refs += 1;
                        type_counts
                            .entry(r.type_str())
                            .and_modify(|c| *c += 1)
                            .or_insert(1);
                        if let Some(base_id) = form_id_from_field(r, b"NAME") {
                            if let Some(base) = by_form_id.get(&base_id) {
                                base_type_counts
                                    .entry(base.type_str())
                                    .and_modify(|c| *c += 1)
                                    .or_insert(1);
                            }
                        }
                    }
                    "LAND" => land = Some(r),
                    other_type => {
                        other
                            .entry(other_type.to_string())
                            .and_modify(|c| *c += 1)
                            .or_insert(1);
                    }
                }
            }
            total_objects += refs;
            if land.is_some() {
                total_land += 1;
            }
            let land_info = match land {
                Some(l) => {
                    let mut parts = Vec::new();
                    for ft in [b"DATA", b"VNML", b"VHGT", b"VCLR", b"VTXT"] {
                        if let Some(f) = l.field(ft) {
                            parts.push(format!(
                                "{}:{}B",
                                String::from_utf8_lossy(ft),
                                f.data.len()
                            ));
                        }
                    }
                    format!("LAND [{}]", parts.join(" "))
                }
                None => "NO LAND".to_string(),
            };
            println!(
                "  grid {g:?}: CELL {} (0x{:08x}) refs={refs} {land_info} other={other:?}",
                cell.edid().unwrap_or_default(),
                cell.form_id
            );
        }
    }
    println!("\n3x3 totals: {total_objects} REFR/ACHR objects, {total_land}/9 cells with LAND");
    println!("ref types: {type_counts:?}");
    let mut base_counts: Vec<(&String, &usize)> = base_type_counts.iter().collect();
    base_counts.sort_by(|a, b| b.1.cmp(a.1));
    println!("base types (top 15):");
    for (t, c) in base_counts.iter().take(15) {
        println!("  {t}: {c}");
    }

    // --- 3b. The persistent cell: the dest door's parent cell
    // 0x00000d74 (grid 0,0) — is it Tamriel's persistent cell?
    // Persistent refs are filed under it regardless of position.
    let persist = index.children_of(0x00000d74);
    let mut pxs: Vec<f32> = Vec::new();
    let mut pys: Vec<f32> = Vec::new();
    let mut pzs: Vec<f32> = Vec::new();
    let mut ptypes: HashMap<String, usize> = HashMap::new();
    for r in persist {
        ptypes
            .entry(r.type_str())
            .and_modify(|c| *c += 1)
            .or_insert(1);
        if let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) {
            pxs.push(pos[0]);
            pys.push(pos[1]);
            pzs.push(pos[2]);
        }
    }
    pxs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    pys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "\n=== persistent cell 0x00000d74: {} child records, types {ptypes:?}",
        persist.len()
    );
    if !pxs.is_empty() {
        println!(
            "  ref positions: x {:.0}..{:.0}, y {:.0}..{:.0} ({} refs with DATA)",
            pxs[0],
            pxs[pxs.len() - 1],
            pys[0],
            pys[pys.len() - 1],
            pxs.len()
        );
    }
    // How many persistent refs fall inside the Riverwood 3x3?
    let min_x = (tx - 1) as f32 * CELL_SIZE_UNITS;
    let max_x = (tx + 2) as f32 * CELL_SIZE_UNITS;
    let min_y = (ty - 1) as f32 * CELL_SIZE_UNITS;
    let max_y = (ty + 2) as f32 * CELL_SIZE_UNITS;
    let mut in_area = 0usize;
    for r in persist {
        if r.type_str() != "REFR" && r.type_str() != "ACHR" {
            continue;
        }
        if let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) {
            if pos[0] >= min_x && pos[0] < max_x && pos[1] >= min_y && pos[1] < max_y {
                in_area += 1;
            }
        }
    }
    println!("  persistent refs inside the Riverwood 3x3 area: {in_area}");

    // --- 4. LAND decode cross-check on the target cell ---
    let target = exterior_by_grid.get(&(tx, ty)).unwrap();
    let recs = index.children_of(target.form_id);
    let Some(land) = recs.iter().find(|r| r.type_str() == "LAND") else {
        println!("\ntarget cell has NO LAND record");
        return;
    };
    let offset = land
        .field(b"DATA")
        .map(|f| i32::from_le_bytes(f.data[0..4].try_into().unwrap()))
        .unwrap_or(0);
    let vhgt = land.field(b"VHGT").map(|f| &f.data);
    println!("\n=== LAND decode for grid ({tx},{ty}) ===");
    println!("DATA height offset: {offset}");
    let Some(vhgt) = vhgt else {
        println!("no VHGT field");
        return;
    };
    println!("VHGT length: {} bytes", vhgt.len());
    println!("VHGT first 64 bytes: {}", hex(&vhgt[..64.min(vhgt.len())]));
    println!(
        "VHGT as int8 first 40: {:?}",
        vhgt[..40.min(vhgt.len())]
            .iter()
            .map(|b| *b as i8)
            .collect::<Vec<_>>()
    );
    println!(
        "VHGT as f32 first 8: {:?}",
        (0..8)
            .map(|i| f32::from_le_bytes(vhgt[i * 4..i * 4 + 4].try_into().unwrap()))
            .collect::<Vec<_>>()
    );
    if vhgt.len() != 1096 {
        println!("unexpected VHGT length — cannot decode");
        return;
    }
    // Skyrim SE VHGT is 1096 bytes, NOT the 4368-byte float grid of
    // older games. Layout hypotheses, scored against object Z:
    //   L1: 4-byte float first + 33x33 int8 deltas + 3 trailing bytes
    //   L2: 33x33 int8 deltas from byte 0 + 7 trailing bytes
    //   scales 1/2/4/8 on the deltas, DATA offset on/off, N-S flip.
    // Ground truth: objects sit on the terrain, and the inn door
    // (XTEL target, Z=-8.3) stands on the ground at (23259.8,
    // -44476.2). The winning decode minimizes median |diff|.
    let first = f32::from_le_bytes(vhgt[0..4].try_into().unwrap());
    println!("VHGT[0..4] as f32: {first}");
    // Collect object (x, y, z) for scoring.
    let mut obj_pts: Vec<(f32, f32, f32)> = Vec::new();
    for r in recs {
        if r.type_str() != "REFR" {
            continue;
        }
        if let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) {
            obj_pts.push((pos[0], pos[1], pos[2]));
        }
    }
    let cell_min_x = tx as f32 * CELL_SIZE_UNITS;
    let cell_min_y = ty as f32 * CELL_SIZE_UNITS;
    let sample_h = |heights: &[f32], x: f32, y: f32, flip_ns: bool| -> Option<f32> {
        let fx = (x - cell_min_x) / 128.0;
        let mut fy = (y - cell_min_y) / 128.0;
        if flip_ns {
            fy = 32.0 - fy;
        }
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
    };
    let score = |heights: &[f32], flip_ns: bool| -> (f32, f32, f32) {
        let mut diffs: Vec<f32> = Vec::new();
        for (x, y, z) in &obj_pts {
            if let Some(h) = sample_h(heights, *x, *y, flip_ns) {
                diffs.push(z - h);
            }
        }
        diffs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = diffs[diffs.len() / 2];
        let mut abs: Vec<f32> = diffs.iter().map(|d| d.abs()).collect();
        abs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med_abs = abs[abs.len() / 2];
        let door_h = sample_h(heights, 23259.8, -44476.2, flip_ns).unwrap_or(f32::NAN);
        (median, med_abs, -8.3 - door_h)
    };
    let mut results: Vec<(f32, String)> = Vec::new();
    for layout in ["L1", "L2"] {
        for scale in [1.0f32, 2.0, 4.0, 8.0] {
            for use_offset in [true, false] {
                for flip_ns in [false, true] {
                    let mut heights = vec![0.0f32; 33 * 33];
                    let mut acc = if layout == "L1" { first } else { 0.0 };
                    for i in 0..33 * 33 {
                        let d = if layout == "L1" {
                            vhgt[4 + i] as i8 as f32
                        } else {
                            vhgt[i] as i8 as f32
                        };
                        acc += d * scale;
                        heights[i] = if use_offset { offset as f32 + acc } else { acc };
                    }
                    let (median, med_abs, door) = score(&heights, flip_ns);
                    results.push((
                        med_abs,
                        format!(
                            "{layout} scale={scale} offset={use_offset} flip_ns={flip_ns}: median diff {median:.1}, median |diff| {med_abs:.1}, door diff {door:.1}"
                        ),
                    ));
                }
            }
        }
    }
    results.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    println!("\nVHGT hypothesis sweep (best 6 of 32):");
    for (_, line) in results.iter().take(6) {
        println!("  {line}");
    }
    // Rebuild the winning decode for the detailed report below.
    // (Recompute by re-running the best combo: parse it back out
    // of the sorted results is fragile — instead, recompute the
    // L1/scale=1/offset/flip=false decode as the documented
    // default and let the sweep line above carry the verdict.)
    let mut heights = vec![0.0f32; 33 * 33];
    let mut acc = first;
    for i in 0..33 * 33 {
        let d = vhgt[4 + i] as i8 as f32;
        acc += d;
        heights[i] = offset as f32 + acc;
    }
    let min_h = heights.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_h = heights.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    println!("terrain heights: min {min_h:.1} max {max_h:.1} (Skyrim units)");
    println!(
        "  corner heights: SW {:.1} SE {:.1} NW {:.1} NE {:.1} centre {:.1}",
        heights[0],
        heights[32],
        heights[32 * 33],
        heights[32 * 33 + 32],
        heights[16 * 33 + 16]
    );
    // Cross-check: sample terrain height under placed objects and
    // compare with the object's Z. Row 0 is the cell's south edge
    // (min Y); col 0 is the west edge (min X). Vertex spacing 128.
    let cell_min_x = tx as f32 * CELL_SIZE_UNITS;
    let cell_min_y = ty as f32 * CELL_SIZE_UNITS;
    let sample = |x: f32, y: f32| -> Option<f32> {
        let fx = (x - cell_min_x) / 128.0;
        let fy = (y - cell_min_y) / 128.0;
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
        let h = h00 * (1.0 - txf) * (1.0 - tyf)
            + h10 * txf * (1.0 - tyf)
            + h01 * (1.0 - txf) * tyf
            + h11 * txf * tyf;
        Some(h)
    };
    let mut checked = 0usize;
    let mut diffs: Vec<f32> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for r in recs {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) else {
            continue;
        };
        let Some(h) = sample(pos[0], pos[1]) else {
            continue;
        };
        let base_id = form_id_from_field(r, b"NAME").unwrap_or(0);
        let base_edid = by_form_id
            .get(&base_id)
            .map(|b| b.edid().unwrap_or_default())
            .unwrap_or_default();
        diffs.push(pos[2] - h);
        checked += 1;
        // Print a few examples of different object kinds.
        let kind = base_edid.chars().take(12).collect::<String>();
        if seen.insert(kind) && seen.len() <= 12 {
            println!(
                "  obj {base_edid} at ({:.0},{:.0},{:.0}) terrain {:.0} diff {:.0}",
                pos[0],
                pos[1],
                pos[2],
                h,
                pos[2] - h
            );
        }
    }
    diffs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if !diffs.is_empty() {
        let median = diffs[diffs.len() / 2];
        let p10 = diffs[diffs.len() / 10];
        let p90 = diffs[diffs.len() * 9 / 10];
        println!(
            "object-Z minus terrain-height over {checked} objects: median {median:.1}, p10 {p10:.1}, p90 {p90:.1} (Skyrim units; 70 = 1 m)"
        );
    }
    // Also print the target cell's XTEL doors (the way back in).
    println!("\n=== XTEL doors in the target exterior cell (way back) ===");
    for r in recs {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(xtel) = r.field(b"XTEL") else {
            continue;
        };
        let base_id = form_id_from_field(r, b"NAME").unwrap_or(0);
        let base_edid = by_form_id
            .get(&base_id)
            .map(|b| b.edid().unwrap_or_default())
            .unwrap_or_default();
        let (pos, _rot) = r
            .field(b"DATA")
            .and_then(|f| parse_placement(&f.data))
            .unwrap_or(([0.0; 3], [0.0; 3]));
        let d = &xtel.data;
        if d.len() == 28 || d.len() == 32 {
            let dest = u32::from_le_bytes(d[0..4].try_into().unwrap());
            let f = |i: usize| f32::from_le_bytes(d[i * 4..i * 4 + 4].try_into().unwrap());
            let parent = index.parent_cell_of(dest);
            let parent_edid = parent
                .and_then(|id| by_form_id.get(&id))
                .map(|c| c.edid().unwrap_or_default())
                .unwrap_or_default();
            println!(
                "  door {base_edid} ref 0x{:08x} at ({:.1},{:.1},{:.1}) -> dest door 0x{dest:08x} in CELL {parent_edid} (0x{:08x}) pos=({:.1},{:.1},{:.1})",
                r.form_id,
                pos[0],
                pos[1],
                pos[2],
                parent.unwrap_or(0),
                f(1),
                f(2),
                f(3)
            );
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}
