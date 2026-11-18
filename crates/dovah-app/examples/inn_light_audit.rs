//! M1 light audit (brief items a + b): the inn's + Sven's house's
//! LIGH refs — base radius vs XRDS override, color, fade, flags,
//! and pairwise overlap (do the fireplace + candle lights overlap
//! too strongly?).
//!
//! Usage: cargo run -p dovah-app --example inn_light_audit

use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const INN_FORM_ID: u32 = 0x000133c6;
const SVEN_FORM_ID: u32 = 0x0001cb84;

fn u32_at(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

fn f32_at(data: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

struct Light {
    ref_id: u32,
    base_id: u32,
    base_edid: String,
    pos: [f32; 3], // Bevy metres
    radius_u: u32,
    xrds_u: Option<u32>,
    color: [u8; 3],
    fade: f32,
    flags: u32,
}

fn audit(plugin: &Plugin, by_form_id: &HashMap<u32, &Record>, cell_id: u32, name: &str) {
    let cell = by_form_id.get(&cell_id).expect("cell record");
    let mut children: Vec<&Record> = Vec::new();
    if let Some(group) = cell.children.as_ref() {
        for g in group.groups() {
            if g.group_type == GroupType::CellChildren {
                for r in g.records() {
                    children.push(r);
                }
            }
        }
    }
    let mut lights: Vec<Light> = Vec::new();
    for r in &children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(name_f) = r.field(b"NAME") else {
            continue;
        };
        let base_id = u32_at(&name_f.data, 0);
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        if base.type_str() != "LIGH" {
            continue;
        }
        let Some(d) = base.field(b"DATA").map(|f| f.data.as_slice()) else {
            continue;
        };
        let radius_u = u32_at(d, 4);
        let c = u32_at(d, 8);
        let color = [(c & 0xff) as u8, ((c >> 8) & 0xff) as u8, ((c >> 16) & 0xff) as u8];
        let flags = u32_at(d, 12);
        let fade = base
            .field(b"FNAM")
            .map(|f| f32_at(&f.data, 0))
            .unwrap_or(1.0);
        let xrds_u = r.field(b"XRDS").map(|f| u32_at(&f.data, 0));
        let (pos, _rot) = r
            .field(b"DATA")
            .map(|f| {
                (
                    [
                        f32_at(&f.data, 0),
                        f32_at(&f.data, 4),
                        f32_at(&f.data, 8),
                    ],
                    [
                        f32_at(&f.data, 12),
                        f32_at(&f.data, 16),
                        f32_at(&f.data, 20),
                    ],
                )
            })
            .unwrap_or(([0.0; 3], [0.0; 3]));
        // Skyrim units -> Bevy metres: (x, z, -y) / 70.
        let pos = [pos[0] / 70.0, pos[2] / 70.0, -pos[1] / 70.0];
        lights.push(Light {
            ref_id: r.form_id,
            base_id,
            base_edid: base.edid().unwrap_or_default(),
            pos,
            radius_u,
            xrds_u,
            color,
            fade,
            flags,
        });
    }
    println!("=== {name} (cell {:#010x}): {} LIGH refs ===", cell_id, lights.len());
    for l in &lights {
        let eff = l.xrds_u.unwrap_or(l.radius_u);
        println!(
            "  ref {:#010x} base {:#010x} {} pos ({:.1},{:.1},{:.1}) radius {}u ({:.1}m){} color #{:02x}{:02x}{:02x} fade {:.2} flags {:#06x}{}{}",
            l.ref_id,
            l.base_id,
            l.base_edid,
            l.pos[0],
            l.pos[1],
            l.pos[2],
            l.radius_u,
            eff as f32 / 70.0,
            match l.xrds_u {
                Some(x) => format!(" XRDS OVERRIDE {}u", x),
                None => String::new(),
            },
            l.color[0],
            l.color[1],
            l.color[2],
            l.fade,
            l.flags,
            if l.flags & 0x1000 != 0 { " SHADOW" } else { "" },
            if l.flags & 0x8 != 0 { " FLICKER" } else { "" },
        );
    }
    // Pairwise overlap: distance vs sum of effective radii.
    println!("  --- overlap (dist < r1+r2) ---");
    let mut overlaps = 0;
    for i in 0..lights.len() {
        for j in i + 1..lights.len() {
            let (a, b) = (&lights[i], &lights[j]);
            let dx = a.pos[0] - b.pos[0];
            let dy = a.pos[1] - b.pos[1];
            let dz = a.pos[2] - b.pos[2];
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            let ra = a.xrds_u.unwrap_or(a.radius_u) as f32 / 70.0;
            let rb = b.xrds_u.unwrap_or(b.radius_u) as f32 / 70.0;
            if dist < ra + rb {
                overlaps += 1;
                println!(
                    "  {:#010x} <-> {:#010x}: dist {:.1}m, r {:.1}+{:.1} = {:.1}m, overlap {:.1}m ({:.0}% of the smaller radius)",
                    a.ref_id,
                    b.ref_id,
                    dist,
                    ra,
                    rb,
                    ra + rb,
                    ra + rb - dist,
                    (ra + rb - dist) / ra.min(rb) * 100.0
                );
            }
        }
    }
    if overlaps == 0 {
        println!("  (no overlapping light pairs)");
    }
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    audit(&plugin, &by_form_id, INN_FORM_ID, "Sleeping Giant Inn");
    audit(&plugin, &by_form_id, SVEN_FORM_ID, "Sven's House");
}
