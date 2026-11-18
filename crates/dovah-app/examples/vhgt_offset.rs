//! DATA-offset question: is LAND DATA (4 bytes, 31 in Riverwood)
//! a height offset added to all VHGT heights, or a constant/flags
//! field? Two checks: DATA values across Tamriel (constant or
//! varying?), and in an area where neighbouring cells have
//! DIFFERENT DATA values, whether adding the offset keeps edges
//! at 0.0 (l1mixed decode) or omitting it does.
//! Also: is D[0] (VHGT byte 4) always 0?

use dovah_esm::Plugin;
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";

fn decode_l1mixed(vhgt: &[u8], offset: i32, use_offset: bool) -> Option<[f32; 33 * 33]> {
    if vhgt.len() != 1096 {
        return None;
    }
    let first = f32::from_le_bytes(vhgt[0..4].try_into().unwrap());
    let d = |i: usize| vhgt[4 + i] as i8 as f32;
    let mut h = [0.0f32; 33 * 33];
    let base = if use_offset { offset as f32 } else { 0.0 };
    let mut col_acc = first;
    for y in 0..33 {
        col_acc += d(y * 33);
        let mut row_acc = col_acc;
        h[y * 33] = base + row_acc;
        for x in 1..33 {
            row_acc += d(y * 33 + x);
            h[y * 33 + x] = base + row_acc;
        }
    }
    Some(h)
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");

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

    // Collect (vhgt, data) for ALL Tamriel cells with LAND.
    let mut raw: HashMap<(i32, i32), (Vec<u8>, i32)> = HashMap::new();
    let mut data_values: HashMap<i32, usize> = HashMap::new();
    let mut d0_values: HashMap<i8, usize> = HashMap::new();
    for (&(gx, gy), cell) in &cells {
        let Some(recs) = children.get(&cell.form_id) else {
            continue;
        };
        let Some(land) = recs.iter().find(|r| r.type_str() == "LAND") else {
            continue;
        };
        let Some(vhgt) = land.field(b"VHGT") else {
            continue;
        };
        if vhgt.data.len() != 1096 {
            continue;
        }
        let offset = land
            .field(b"DATA")
            .and_then(|f| f.data.get(0..4))
            .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
            .unwrap_or(0);
        *data_values.entry(offset).or_insert(0) += 1;
        *d0_values.entry(vhgt.data[4] as i8).or_insert(0) += 1;
        raw.insert((gx, gy), (vhgt.data.clone(), offset));
    }
    println!("Tamriel cells with 1096-byte VHGT: {}", raw.len());
    println!("DATA value histogram (top 10): {:?}", {
        let mut v: Vec<_> = data_values.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        v.into_iter().take(10).collect::<Vec<_>>()
    });
    println!("D[0] (VHGT byte 4) histogram: {:?}", {
        let mut v: Vec<_> = d0_values.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        v.into_iter().take(5).collect::<Vec<_>>()
    });

    // Edge test over the whole of Tamriel, with and without offset.
    for use_offset in [true, false] {
        let mut lands: HashMap<(i32, i32), [f32; 33 * 33]> = HashMap::new();
        for (k, (vhgt, offset)) in &raw {
            if let Some(h) = decode_l1mixed(vhgt, *offset, use_offset) {
                lands.insert(*k, h);
            }
        }
        let mut worst = 0.0f32;
        let mut n_edges = 0usize;
        let mut n_bad = 0usize;
        for (&(gx, gy), h) in &lands {
            if let Some(e) = lands.get(&(gx + 1, gy)) {
                for y in 0..33 {
                    let d = (h[y * 33 + 32] - e[y * 33]).abs();
                    worst = worst.max(d);
                    n_edges += 1;
                    if d > 0.5 {
                        n_bad += 1;
                    }
                }
            }
            if let Some(n) = lands.get(&(gx, gy + 1)) {
                for x in 0..33 {
                    let d = (h[32 * 33 + x] - n[x]).abs();
                    worst = worst.max(d);
                    n_edges += 1;
                    if d > 0.5 {
                        n_bad += 1;
                    }
                }
            }
        }
        println!(
            "use_offset={use_offset}: {n_edges} shared edge vertices, worst mismatch {worst:.1}, vertices off by >0.5: {n_bad}"
        );
    }
}
