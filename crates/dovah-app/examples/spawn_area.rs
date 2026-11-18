//! What is around the inn-door spawn point? List every REFR/ACHR
//! within 1500 Skyrim units (~21 m) of the exterior XTEL arrival
//! (23259.8, -44476.2, -8.3), with base EDID, model, position and
//! rotation — to find out which mesh the M9 screenshot camera is
//! staring at, and where the village is relative to the spawn.

use dovah_esm::Plugin;
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const SPAWN: [f32; 3] = [23259.8, -44476.2, -8.3];
const RADIUS: f32 = 1500.0;

// (distance, ref form id, base type, base edid, pos, rot, model)
type NearbyRef = (f32, u32, String, String, [f32; 3], [f32; 3], String);

fn parse_placement(data: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    if data.len() != 24 {
        return None;
    }
    let f = |i: usize| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    Some(([f(0), f(1), f(2)], [f(3), f(4), f(5)]))
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut by_form_id: HashMap<u32, &dovah_esm::Record> = HashMap::new();
    for r in plugin.all_records() {
        by_form_id.insert(r.form_id, r);
    }
    let mut near: Vec<NearbyRef> = Vec::new();
    for r in plugin.all_records() {
        let kind = r.type_str();
        if kind != "REFR" && kind != "ACHR" {
            continue;
        }
        let Some((pos, rot)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) else {
            continue;
        };
        let dx = pos[0] - SPAWN[0];
        let dy = pos[1] - SPAWN[1];
        let dz = pos[2] - SPAWN[2];
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();
        if dist > RADIUS {
            continue;
        }
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
        let base = by_form_id.get(&base_id);
        let (btype, edid, model) = match base {
            Some(b) => (
                b.type_str(),
                b.edid().unwrap_or_default(),
                b.field(b"MODL").map(|f| f.as_str()).unwrap_or_default(),
            ),
            None => ("?".to_string(), "?".to_string(), String::new()),
        };
        near.push((dist, r.form_id, btype, edid, pos, rot, model));
    }
    near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    println!("refs within {} units of the spawn: {}", RADIUS, near.len());
    for (dist, form_id, btype, edid, pos, rot, model) in near {
        println!(
            "{dist:7.0}u ref 0x{form_id:08x} {btype} {edid} pos=({:.0},{:.0},{:.0}) rotZ={:.3} {model}",
            pos[0], pos[1], pos[2], rot[2]
        );
    }
}
