//! Light-falloff calibration probe: for the Sleeping Giant Inn,
//! compute the linear-falloff attenuation field (Skyrim's formula:
//! Fade x max(0, 1 - d/Radius)) over a grid of sample points, and
//! the same field under Bevy's old inverse-square law, so the new
//! intensity constant K (intensity = K x Fade) can be picked from
//! the game's own geometry instead of guessed.
//!
//! Prints: mean / p5 / p50 / p95 / max attenuation under each law,
//! the value at the load door, and the K that yields a chosen mean
//! lux. Target mean lux ~350 (pixel ~0.22 sRGB at INDOOR exposure,
//! matching the M16 room level).
//!
//! Usage: cargo run -p dovah-app --example light_level_probe

use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const INN_FORM_ID: u32 = 0x000133c6;
const LOAD_DOOR_REF: u32 = 0x00013419;
const TARGET_MEAN_LUX: f32 = 350.0;

fn f32_at(data: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

fn u32_at(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

/// Skyrim (x, y, z) units -> Bevy metres (x, z, -y) / 70.
fn to_bevy(p: (f32, f32, f32)) -> (f32, f32, f32) {
    (p.0 / 70.0, p.2 / 70.0, -p.1 / 70.0)
}

fn dist(a: (f32, f32, f32), b: (f32, f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt()
}

struct Light {
    edid: String,
    pos: (f32, f32, f32),
    radius_m: f32,
    fade: f32,
}

fn percentile(sorted: &[f32], p: f32) -> f32 {
    let idx = ((sorted.len() - 1) as f32 * p).round() as usize;
    sorted[idx]
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    let mut children: Vec<&Record> = Vec::new();
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        if matches!(
            g.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        ) && g.label_form_id() == Some(INN_FORM_ID)
        {
            children.extend(g.records());
        }
        stack.extend(g.subgroups());
    }
    let mut lights: Vec<Light> = Vec::new();
    let mut door_pos: Option<(f32, f32, f32)> = None;
    let mut min = (f32::MAX, f32::MAX, f32::MAX);
    let mut max = (f32::MIN, f32::MIN, f32::MIN);
    for r in &children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(df) = r.field(b"DATA") else { continue };
        if df.data.len() < 12 {
            continue;
        }
        let pos = to_bevy((
            f32_at(&df.data, 0),
            f32_at(&df.data, 4),
            f32_at(&df.data, 8),
        ));
        min = (min.0.min(pos.0), min.1.min(pos.1), min.2.min(pos.2));
        max = (max.0.max(pos.0), max.1.max(pos.1), max.2.max(pos.2));
        if r.form_id == LOAD_DOOR_REF {
            door_pos = Some(pos);
        }
        let Some(name_f) = r.field(b"NAME") else {
            continue;
        };
        if name_f.data.len() != 4 {
            continue;
        }
        let base_id = u32::from_le_bytes(name_f.data[0..4].try_into().unwrap());
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        if base.type_str() != "LIGH" {
            continue;
        }
        let Some(d) = base.field(b"DATA").map(|f| f.data.as_slice()) else {
            continue;
        };
        let radius_m = u32_at(d, 4) as f32 / 70.0;
        let fade = base
            .field(b"FNAM")
            .filter(|f| f.data.len() == 4)
            .map(|f| f32_at(&f.data, 0))
            .unwrap_or(1.0);
        lights.push(Light {
            edid: base.edid().unwrap_or_default().to_string(),
            pos,
            radius_m,
            fade,
        });
    }
    println!("inn lights: {}", lights.len());
    for l in &lights {
        println!(
            "  {:<28} pos=({:6.2},{:5.2},{:6.2}) R={:.2}m fade={:.2}",
            l.edid, l.pos.0, l.pos.1, l.pos.2, l.radius_m, l.fade
        );
    }
    println!(
        "room bounds x [{:.1},{:.1}] y [{:.1},{:.1}] z [{:.1},{:.1}]",
        min.0, max.0, min.1, max.1, min.2, max.2
    );
    if let Some(d) = door_pos {
        println!(
            "load door 0x{:08x} bevy pos=({:.2},{:.2},{:.2})",
            LOAD_DOOR_REF, d.0, d.1, d.2
        );
    }

    // Sample at the actual REFR positions (real surfaces), not a grid:
    // the cell's raw position bounds include outliers (y to 19.5m),
    // so a grid over them samples mostly empty space outside the room.
    let mut linear: Vec<f32> = Vec::new();
    let mut inverse: Vec<f32> = Vec::new();
    for r in &children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(df) = r.field(b"DATA") else { continue };
        if df.data.len() < 12 {
            continue;
        }
        let p = to_bevy((
            f32_at(&df.data, 0),
            f32_at(&df.data, 4),
            f32_at(&df.data, 8),
        ));
        // Sample at the object's height, clamped to the room (0.3..4.5m).
        let p = (p.0, p.1.clamp(0.3, 4.5), p.2);
        let mut lin = 0.0f32;
        let mut inv = 0.0f32;
        for l in &lights {
            let d = dist(p, l.pos);
            // Skyrim linear: Fade x max(0, 1 - d/R).
            lin += l.fade * (1.0 - d / l.radius_m).max(0.0);
            // Old Bevy law: I = 8000 x R^2 (fireplace x2), irradiance =
            // I / d^2 x window, window = saturate(1 - (d^2/R^2)^2)^2.
            // (The saturate is load-bearing: without it the window
            // EXPLODES past R instead of cutting off.)
            let d2 = d.max(0.3).powi(2);
            let r2 = l.radius_m.powi(2);
            let window = (1.0 - (d2 / r2).powi(2)).clamp(0.0, 1.0).powi(2);
            let i = if l.edid.contains("Fire") { 2.0 } else { 1.0 } * 8000.0 * r2;
            inv += i / d2 * window;
        }
        linear.push(lin);
        inverse.push(inv);
    }
    linear.sort_by(|a, b| a.partial_cmp(b).unwrap());
    inverse.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let lin_mean = linear.iter().sum::<f32>() / linear.len() as f32;
    let inv_mean = inverse.iter().sum::<f32>() / inverse.len() as f32;
    println!("\nsamples: {} points", linear.len());
    println!("LINEAR attenuation (dimensionless, x Fade): mean={:.3} p5={:.3} p50={:.3} p95={:.3} max={:.3}",
        lin_mean, percentile(&linear, 0.05), percentile(&linear, 0.5),
        percentile(&linear, 0.95), linear[linear.len() - 1]);
    println!(
        "  -> K for mean {:.0} lux: {:.0}",
        TARGET_MEAN_LUX,
        TARGET_MEAN_LUX / lin_mean
    );
    println!(
        "  -> max lux at that K: {:.0} (pixel ~{:.2})",
        TARGET_MEAN_LUX / lin_mean * linear[linear.len() - 1],
        TARGET_MEAN_LUX / lin_mean * linear[linear.len() - 1] * 0.00062
    );
    println!("INVERSE-SQUARE irradiance (lux, old law): mean={:.0} p5={:.0} p50={:.0} p95={:.0} max={:.0}",
        inv_mean, percentile(&inverse, 0.05), percentile(&inverse, 0.5),
        percentile(&inverse, 0.95), inverse[inverse.len() - 1]);
    println!(
        "  shape ratio max/mean: linear {:.1}x, inverse-square {:.1}x",
        linear[linear.len() - 1] / lin_mean,
        inverse[inverse.len() - 1] / inv_mean
    );

    // The load door point under both laws.
    if let Some(d) = door_pos {
        let door_point = (d.0, 1.7, d.2);
        let mut lin = 0.0f32;
        let mut inv = 0.0f32;
        for l in &lights {
            let dd = dist(door_point, l.pos);
            lin += l.fade * (1.0 - dd / l.radius_m).max(0.0);
            let d2 = dd.max(0.3).powi(2);
            let r2 = l.radius_m.powi(2);
            let window = (1.0 - (d2 / r2).powi(2)).clamp(0.0, 1.0).powi(2);
            let i = if l.edid.contains("Fire") { 2.0 } else { 1.0 } * 8000.0 * r2;
            inv += i / d2 * window;
        }
        let k = TARGET_MEAN_LUX / lin_mean;
        println!("\nload door at eye height: linear att={:.3} -> {:.0} lux at K={:.0} (pixel ~{:.2}); old law {:.0} lux (pixel ~{:.2})",
            lin, k * lin, k, k * lin * 0.00062, inv, inv * 0.00062);
    }
}
