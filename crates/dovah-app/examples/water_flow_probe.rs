//! Water flow probe: which WATR field encodes flow direction?
//!
//! Diffs the directional river flow types (NE/NW/SE) plus the
//! still DefaultRiverWater across DNAM + GNAM + NAM0 + NAM1.
//! The field(s) that differ ONLY between the directional
//! variants are the flow encoding; fields identical across all
//! four are shared water params.
//!
//! Usage: cargo run -p dovah-app --example water_flow_probe

use dovah_esm::{Plugin, Record};

const ESM_PATH: &str = "test-data/Skyrim.esm";

/// (form ID, EDID)
const FLOW_TYPES: [(u32, &str); 4] = [
    (0x000e717c, "RiverWaterFlowNE"),
    (0x001063aa, "RiverWaterFlowNW"),
    (0x00108ac7, "RiverWaterFlowSE"),
    (0x00105ce1, "DefaultRiverWater"),
];

fn f32s(data: &[u8]) -> Vec<f32> {
    let (chunks, _) = data.as_chunks::<4>();
    chunks.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

fn dump(w: &Record, field_type: &[u8; 4]) -> Option<Vec<f32>> {
    w.field(field_type).map(|f| f32s(&f.data))
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut by_form_id = std::collections::HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }

    for ft in [b"DNAM", b"GNAM", b"NAM0", b"NAM1"] {
        println!("=== {} ===", String::from_utf8_lossy(ft));
        let mut rows: Vec<(String, Vec<f32>)> = Vec::new();
        for (id, edid) in FLOW_TYPES {
            let Some(w) = by_form_id.get(&id) else {
                println!("  {edid} (0x{id:08x}) NOT FOUND");
                continue;
            };
            match dump(w, ft) {
                Some(v) => rows.push((edid.to_string(), v)),
                None => println!(
                    "  {edid}: no {ft:?} field",
                    ft = String::from_utf8_lossy(ft)
                ),
            }
        }
        if rows.is_empty() {
            continue;
        }
        let n = rows[0].1.len();
        // Print only the offsets where at least one row differs.
        let mut any_diff = false;
        for i in 0..n {
            let first = rows[0].1[i];
            if rows
                .iter()
                .any(|(_, v)| v.get(i).copied().unwrap_or(f32::NAN) != first)
            {
                any_diff = true;
                let vals: Vec<String> = rows
                    .iter()
                    .map(|(e, v)| {
                        format!(
                            "{}={}",
                            e,
                            v.get(i).map(|x| format!("{x:.6}")).unwrap_or("—".into())
                        )
                    })
                    .collect();
                println!("  @{}: {}", i * 4, vals.join("  "));
            }
        }
        if !any_diff {
            println!("  identical across all {} types ({} floats)", rows.len(), n);
        }
        // Also print the full first row for the short fields.
        if n <= 3 {
            for (e, v) in &rows {
                println!("  {e}: {v:?}");
            }
        }
    }

    // Also: the other flow pairs — CreekWaterFlowSE vs SW, and
    // WRWaterFlow vs DefaultRiverWater — same diff on DNAM only.
    println!("\n=== DNAM diff: CreekWaterFlowSE vs CreekWaterFlowSW ===");
    let a = by_form_id[&0x00106a20u32];
    let b = by_form_id[&0x00106a24u32];
    let da = dump(a, b"DNAM").unwrap();
    let db = dump(b, b"DNAM").unwrap();
    for i in 0..da.len() {
        if da[i] != db[i] {
            println!("  @{}: SE={} SW={}", i * 4, da[i], db[i]);
        }
    }
    println!("(no lines above = DNAM identical between SE and SW)");
}
