//! Archery PROJ probe: the AMMO DATA @0 field is a projectile
//! form ID (0x0003be11.. for the tier arrows). The arrow's flight
//! parameters (speed, gravity, range) live in the PROJ record,
//! not on the bow or the arrow. Dump every PROJ field + f32 reads.
//!
//! Also re-confirms the bow discriminator: WEAP DNAM u32 @0
//! (animation type) for bows vs melee weapons.
//!
//! Usage: cargo run -p dovah-esm --example archery_proj_probe

use dovah_esm::Plugin;
use std::collections::HashMap;
use std::path::Path;

const ESM_PATH: &str = "test-data/Skyrim.esm";

// AMMO DATA @0 values from archery_probe (projectile form IDs).
const PROJ: &[(&str, u32)] = &[
    ("IronArrow", 0x0003be11),
    ("SteelArrow", 0x0003be12),
    ("OrcishArrow", 0x0003be13),
    ("DwarvenArrow", 0x0003be14),
    ("ElvenArrow", 0x0003be15),
    ("GlassArrow", 0x0003be16),
    ("EbonyArrow", 0x0003be17),
    ("DaedricArrow", 0x0003be18),
    ("FalmerArrow", 0x0003be19),
    ("ForswornArrow", 0x000ceea1),
    ("NordHeroArrow", 0x000eafe0),
];

const WEAPONS: &[&str] = &[
    "HuntingBow",
    "LongBow",
    "ImperialBow",
    "DaedricBow",
    "FalmerBow",
    "IronSword",
    "IronDagger",
    "IronGreatsword",
    "IronBattleaxe",
    "IronWarAxe",
    "IronMace",
];

fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn f32_at(data: &[u8], off: usize) -> Option<f32> {
    let b: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
    Some(f32::from_le_bytes(b))
}

fn u32_at(data: &[u8], off: usize) -> Option<u32> {
    let b: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(b))
}

fn main() {
    let plugin = Plugin::open(Path::new(ESM_PATH)).expect("load Skyrim.esm");
    let mut by_form_id: HashMap<u32, &dovah_esm::Record> = HashMap::new();
    let mut by_edid: HashMap<String, &dovah_esm::Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
        if let Some(edid) = record.edid() {
            by_edid.insert(edid, record);
        }
    }

    println!("=== PROJ records ===");
    for (arrow, proj_id) in PROJ {
        let Some(r) = by_form_id.get(proj_id) else {
            println!("{arrow} PROJ 0x{proj_id:08x}: NOT FOUND");
            continue;
        };
        println!(
            "{arrow} PROJ 0x{proj_id:08x} type={} edid={}",
            r.type_str(),
            r.edid().unwrap_or_default()
        );
        for field in &r.fields {
            let ty = field.type_str();
            if ty == "EDID" {
                continue;
            }
            println!("  {ty}({}B): {}", field.data.len(), hex(&field.data));
            if field.data.len() >= 4 {
                let mut f32s = vec![];
                let mut off = 0;
                while off + 4 <= field.data.len() {
                    if let Some(v) = f32_at(&field.data, off) {
                        if v.is_finite() && v != 0.0 && v.abs() < 100000.0 {
                            f32s.push(format!("@{off}={v}"));
                        }
                    }
                    off += 4;
                }
                if !f32s.is_empty() {
                    println!("    f32s: {}", f32s.join(" "));
                }
            }
        }
    }

    println!("\n=== WEAP DNAM u32 @0 (animation type) ===");
    for edid in WEAPONS {
        let Some(r) = by_edid.get(*edid) else {
            println!("{edid}: NOT FOUND");
            continue;
        };
        let anim = r
            .field(b"DNAM")
            .and_then(|f| u32_at(&f.data, 0));
        println!("{edid} 0x{:08x} anim_type={anim:?}", r.form_id);
    }
}
