//! M10 probe: sample the actual VCLR vertex colors for the
//! Riverwood centre cell (5,-11) — is the pale ground in the
//! screenshots what the data says, or a pipeline bug?
//!
//! Prints VCLR field length, per-channel min/max/mean, and a
//! coarse 8x8 ASCII map of luminance, plus the same for the
//! neighbouring cells. Usage:
//! `cargo run -p dovah-app --example vclr_check`

use dovah_esm::{Group, GroupType, Plugin, Record};
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

fn is_interior(cell: &Record) -> bool {
    cell.field(b"DATA")
        .and_then(|f| f.data.first())
        .map(|b| b & 0x01 != 0)
        .unwrap_or(false)
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    // Cell form ID -> child records.
    let mut children: HashMap<u32, Vec<&Record>> = HashMap::new();
    fn scan<'a>(group: &'a Group, children: &mut HashMap<u32, Vec<&'a Record>>) {
        if matches!(
            group.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        ) {
            if let Some(cell_id) = group.label_form_id() {
                let entry = children.entry(cell_id).or_default();
                for r in group.records() {
                    entry.push(r);
                }
            }
        }
        for subgroup in group.subgroups() {
            scan(subgroup, children);
        }
    }
    for group in &plugin.groups {
        scan(group, &mut children);
    }
    // Exterior grid map by median ref position.
    let mut exterior_by_grid: HashMap<(i32, i32), &Record> = HashMap::new();
    for record in plugin.all_records() {
        if record.type_str() != "CELL" || is_interior(record) {
            continue;
        }
        let recs = children
            .get(&record.form_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
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
            continue;
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let gx = (xs[xs.len() / 2] / CELL_SIZE_UNITS).floor() as i32;
        let gy = (ys[ys.len() / 2] / CELL_SIZE_UNITS).floor() as i32;
        exterior_by_grid.insert((gx, gy), record);
    }

    for (gx, gy) in [(5, -11), (4, -11), (6, -11), (5, -12), (5, -10)] {
        let Some(cell) = exterior_by_grid.get(&(gx, gy)) else {
            println!("grid ({gx},{gy}): NO CELL");
            continue;
        };
        let recs = children
            .get(&cell.form_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let Some(land) = recs.iter().find(|r| r.type_str() == "LAND") else {
            println!("grid ({gx},{gy}): NO LAND");
            continue;
        };
        let Some(vclr) = land.field(b"VCLR") else {
            println!("grid ({gx},{gy}): LAND has NO VCLR field");
            continue;
        };
        let data = &vclr.data;
        println!(
            "\ngrid ({gx},{gy}): VCLR len {} bytes (expect {} for 33x33x3)",
            data.len(),
            33 * 33 * 3
        );
        if data.len() < 33 * 33 * 3 {
            continue;
        }
        let mut min = [255u8; 3];
        let mut max = [0u8; 3];
        let mut sum = [0u64; 3];
        for i in 0..33 * 33 {
            for c in 0..3 {
                let v = data[i * 3 + c];
                min[c] = min[c].min(v);
                max[c] = max[c].max(v);
                sum[c] += v as u64;
            }
        }
        let n = (33 * 33) as u64;
        println!(
            "  R min {} max {} mean {:.0} | G min {} max {} mean {:.0} | B min {} max {} mean {:.0}",
            min[0],
            max[0],
            sum[0] as f64 / n as f64,
            min[1],
            max[1],
            sum[1] as f64 / n as f64,
            min[2],
            max[2],
            sum[2] as f64 / n as f64
        );
        // Coarse luminance map (8x8, one char per ~4 vertices).
        println!("  luminance map (dark=' ' .. bright='#'):");
        for row in (0..8).rev() {
            let mut line = String::from("    ");
            for col in 0..8 {
                let mut acc = 0u32;
                let mut count = 0u32;
                for dy in 0..4 {
                    for dx in 0..4 {
                        let x = col * 4 + dx;
                        let y = row * 4 + dy;
                        if x < 33 && y < 33 {
                            let i = (y * 33 + x) * 3;
                            let lum =
                                (data[i] as u32 * 3 + data[i + 1] as u32 * 6 + data[i + 2] as u32)
                                    / 10;
                            acc += lum;
                            count += 1;
                        }
                    }
                }
                let lum = acc / count.max(1);
                let ch = match lum {
                    0..=32 => ' ',
                    33..=64 => '.',
                    65..=96 => ':',
                    97..=128 => '*',
                    129..=160 => 'o',
                    161..=192 => 'O',
                    193..=224 => '#',
                    _ => '@',
                };
                line.push(ch);
            }
            println!("{line}");
        }
        // Sample a few raw triples (corners + centre).
        for (label, i) in [
            ("SW", 0),
            ("SE", 32),
            ("NW", 32 * 33),
            ("NE", 32 * 33 + 32),
            ("centre", 16 * 33 + 16),
        ] {
            println!(
                "  {label}: RGB=({},{},{})",
                data[i * 3],
                data[i * 3 + 1],
                data[i * 3 + 2]
            );
        }
    }
}
