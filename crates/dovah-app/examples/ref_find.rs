//! Find REFRs/ACHRs whose base EDID contains a substring, and print
//! their positions in Skyrim units and Bevy metres — to locate a
//! mesh seen in a render (M13: the white flat plane above the inn
//! sign) from the ESM instead of guessing.
//!
//! Usage: cargo run -p dovah-app --example ref_find -- <substring>
//! [substring...] — every substring is matched (case-insensitive).

use dovah_esm::Plugin;
use std::collections::HashMap;
use std::env;

const ESM_PATH: &str = "test-data/Skyrim.esm";

fn parse_placement(data: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    if data.len() != 24 {
        return None;
    }
    let f = |i: usize| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    Some(([f(0), f(1), f(2)], [f(3), f(4), f(5)]))
}

fn main() {
    let needles: Vec<String> = env::args().skip(1).map(|s| s.to_lowercase()).collect();
    if needles.is_empty() {
        eprintln!("usage: ref_find <edid-substring> [substring...]");
        return;
    }
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut by_form_id: HashMap<u32, &dovah_esm::Record> = HashMap::new();
    for r in plugin.all_records() {
        by_form_id.insert(r.form_id, r);
    }
    let mut hits = 0;
    for r in plugin.all_records() {
        let kind = r.type_str();
        if kind != "REFR" && kind != "ACHR" {
            continue;
        }
        let Some((pos, rot)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) else {
            continue;
        };
        let base_id = r
            .field(b"NAME")
            .and_then(|f| {
                if f.data.len() == 4 {
                    Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        let edid = base.edid().unwrap_or_default();
        let edid_lc = edid.to_lowercase();
        if !needles.iter().any(|n| edid_lc.contains(n.as_str())) {
            continue;
        }
        let model = base.field(b"MODL").map(|f| f.as_str()).unwrap_or_default();
        // Bevy metres: x = X/70, y = Z/70, z = -Y/70.
        println!(
            "ref 0x{:08x} {} {} pos=({:.0},{:.0},{:.0}) rotZ={:.3} bevy=({:.1},{:.1},{:.1}) {}",
            r.form_id,
            base.type_str(),
            edid,
            pos[0],
            pos[1],
            pos[2],
            rot[2],
            pos[0] / 70.0,
            pos[2] / 70.0,
            -pos[1] / 70.0,
            model
        );
        hits += 1;
    }
    println!("{hits} refs matched");
}
