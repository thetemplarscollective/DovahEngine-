//! v0.4.6 phone-feedback probe (drop diagnosis): "Items dont
//! show up when you drop em." The v0.4.6 log proves the entity
//! spawns (1 entities, ref 0xff000000) — so the item is
//! invisible, not missing. Two candidate causes, both tested
//! here at the data level (the render proof uses --drop-test):
//!
//! Part 1 — SINKING: `drop_item` spawns the ground model's mesh
//! ORIGIN exactly at feet level, with no rest-on-floor logic.
//! If the model's geometry extends below its origin, the item
//! sinks into the floor. For each of 4 items (big weapon, tiny
//! ingredient, armor ground model, small potion — found by EDID
//! substring, since FULL is a localized .strings ID, not inline
//! text) this prints the ground model's rendered bounds in
//! metres, via the SAME `nif_mesh_to_bevy` the drop path uses:
//! min Y = how far the model sinks below the drop origin; max Y
//! = how far it rises above it. Also prints each base form ID
//! (what `--drop-test` needs).
//! Part 2 — VIEWPOINTS: prints the inn + exterior default eye
//! positions and the predicted drop points (feet + flattened
//! forward * 0.8), so the --drop-test renders can aim a lowered
//! --camera at the exact drop spot. Replicates
//! `screenshot_viewpoint` / `spawn_viewpoint` (scene.rs, private
//! module) and `to_bevy_position` (coords.rs, private module).
//!
//! Usage: cargo run -p dovah-app --example drop_bounds_probe

use bevy::math::Vec3;
use bevy::mesh::{Mesh, VertexAttributeValues};
use dovah_app::assets::AssetProvider;
use dovah_app::cell::{find_xtel_spawn, load_cell};
use dovah_esm::{Plugin, Record};
use dovah_nif::NifFile;
use std::collections::HashMap;
use std::path::Path;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const INN_EDID: &str = "RiverwoodSleepingGiantInn";

/// Skyrim units per metre.
const UPM: f32 = 70.0;
/// Eye height above the feet (player.rs).
const EYE_HEIGHT: f32 = 1.7;

/// Skyrim [x, y, z] units -> Bevy metres. Replicates
/// coords::to_bevy_position: Bevy = (x/70, z/70, -y/70).
fn to_bevy_position(p: [f32; 3]) -> Vec3 {
    Vec3::new(p[0] / UPM, p[2] / UPM, -p[1] / UPM)
}

/// Skyrim direction -> Bevy direction (no scaling).
fn to_bevy_direction(d: [f32; 3]) -> Vec3 {
    Vec3::new(d[0], d[2], -d[1])
}

fn model_string(record: &Record, code: &[u8; 4]) -> Option<String> {
    record.field(code).map(|f| {
        String::from_utf8_lossy(&f.data)
            .trim_end_matches('\0')
            .to_string()
    })
}

/// The catalog's ground-model rule (inventory.rs): non-ARMO
/// records use MODL; ARMO uses MOD2 (male) then MOD4 (female).
/// The player is male by default, so MOD2 wins for armor.
fn ground_model(record: &Record) -> Option<String> {
    if record.type_str() == "ARMO" {
        model_string(record, b"MOD2").or_else(|| model_string(record, b"MOD4"))
    } else {
        model_string(record, b"MODL")
    }
}

/// Rendered bounds of one Bevy mesh (metres), from the POSITION
/// attribute. Returns (min, max).
fn mesh_bounds(mesh: &Mesh) -> Option<(Vec3, Vec3)> {
    let VertexAttributeValues::Float32x3(pos) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)? else {
        return None;
    };
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for p in pos {
        let v = Vec3::from(*p);
        min = min.min(v);
        max = max.max(v);
    }
    if min.x.is_infinite() {
        return None;
    }
    Some((min, max))
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let all = plugin.all_records();
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in &all {
        by_form_id.insert(record.form_id, *record);
    }
    println!("records indexed: {}", by_form_id.len());
    let mut provider = AssetProvider::new(Path::new("test-data"));

    // ---- Part 1: ground-model bounds for 4 items ----
    // Found by EDID substring: FULL is a localized .strings ID
    // (u32), not inline text, so a FULL-name search matches
    // nothing. Each entry is (EDID needle, expected type).
    println!("\n=== Part 1: ground-model bounds (drop origin = feet level) ===");
    let wanted = [
        ("IronSword", "WEAP"),
        ("NordicBarnacle", "INGR"),
        ("IronHelmet", "ARMO"),
        ("PotionOfMinorHealing", "ALCH"),
    ];
    for (needle, want_type) in wanted {
        let found = all.iter().find(|r| {
            r.type_str() == want_type && r.edid().as_deref().unwrap_or("").contains(needle)
        });
        let Some(record) = found else {
            println!("{needle} ({want_type}): NOT FOUND by EDID");
            continue;
        };
        let name = record.edid().unwrap_or_default();
        let Some(model) = ground_model(record) else {
            println!(
                "{name} (base {:#010x}, {}): NO ground model",
                record.form_id,
                record.type_str()
            );
            continue;
        };
        let nif_path = format!("meshes\\{model}");
        let Some(bytes) = provider.read_file(&nif_path) else {
            println!(
                "{name} (base {:#010x}): mesh not found: {nif_path}",
                record.form_id
            );
            continue;
        };
        let Ok(nif) = NifFile::from_bytes(&bytes) else {
            println!("{name} (base {:#010x}): NIF parse failed", record.form_id);
            continue;
        };
        let sub_meshes = nif.meshes();
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut converted = 0;
        for sub in &sub_meshes {
            let Some(bevy_mesh) = dovah_app::convert::nif_mesh_to_bevy(sub) else {
                continue;
            };
            let Some((lo, hi)) = mesh_bounds(&bevy_mesh) else {
                continue;
            };
            min = min.min(lo);
            max = max.max(hi);
            converted += 1;
        }
        if converted == 0 {
            println!(
                "{name} (base {:#010x}, {}): {} sub-meshes, 0 convert",
                record.form_id,
                record.type_str(),
                sub_meshes.len()
            );
            continue;
        }
        let height = max.y - min.y;
        println!(
            "{name} (base {:#010x}, {}): ground model {model}",
            record.form_id,
            record.type_str()
        );
        println!(
            "  sub-meshes {} ({} converted), bounds min [{:.3}, {:.3}, {:.3}] max [{:.3}, {:.3}, {:.3}], height {:.3} m",
            sub_meshes.len(),
            converted,
            min.x,
            min.y,
            min.z,
            max.x,
            max.y,
            max.z,
            height
        );
        if min.y < -0.001 {
            println!(
                "  SINKS {:.3} m below the drop origin ({:.0}% of its height buried)",
                -min.y,
                -min.y / height * 100.0
            );
        } else {
            println!("  sits ON the origin (min Y {:.3} m) — no sinking", min.y);
        }
    }

    // ---- Part 2: viewpoints + predicted drop points ----
    println!("\n=== Part 2: viewpoints + predicted drop points ===");
    // Inn: screenshot_viewpoint (no spawn point on the default
    // inn load) — replicates scene.rs.
    let inn = load_cell(&plugin, INN_EDID, None).expect("load inn interior");
    let (min_sky, max_sky) = inn.position_bounds();
    let min = Vec3::new(min_sky[0] / UPM, min_sky[2] / UPM, -max_sky[1] / UPM);
    let max = Vec3::new(max_sky[0] / UPM, max_sky[2] / UPM, -min_sky[1] / UPM);
    let size = max - min;
    let center = to_bevy_position(inn.median_position());
    let long_axis = if size.x >= size.z { Vec3::X } else { Vec3::Z };
    let back = long_axis * size.x.max(size.z) * 0.12;
    let eye = center - back + Vec3::new(0.0, EYE_HEIGHT, 0.0);
    let look_at = center + back * 0.5 + Vec3::new(0.0, 0.2, 0.0);
    let feet = eye - Vec3::new(0.0, EYE_HEIGHT, 0.0);
    let forward = (look_at - eye).normalize();
    let flat = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let drop = feet + flat * 0.8;
    println!(
        "inn default eye: [{:.2}, {:.2}, {:.2}]",
        eye.x, eye.y, eye.z
    );
    println!(
        "inn look_at: [{:.2}, {:.2}, {:.2}]",
        look_at.x, look_at.y, look_at.z
    );
    println!(
        "inn predicted drop point: [{:.2}, {:.2}, {:.2}]",
        drop.x, drop.y, drop.z
    );
    println!(
        "inn lowered --camera: --camera {:.2},{:.2},{:.2}:{:.2},{:.2},{:.2}",
        eye.x, eye.y, eye.z, drop.x, drop.y, drop.z
    );

    // Exterior: the arrival spawn comes from the inn's first
    // door's XTEL (the load door 0x00013419), as the game
    // resolves it.
    let Some(spawn) = find_xtel_spawn(&plugin, INN_EDID) else {
        println!("exterior: no XTEL spawn found for {INN_EDID}");
        return;
    };
    let ext_eye = to_bevy_position(spawn.position) + Vec3::new(0.0, EYE_HEIGHT, 0.0);
    let forward_sky = [spawn.yaw.sin(), spawn.yaw.cos(), 0.0];
    let ext_forward = to_bevy_direction(forward_sky);
    let ext_feet = ext_eye - Vec3::new(0.0, EYE_HEIGHT, 0.0);
    let ext_flat = Vec3::new(ext_forward.x, 0.0, ext_forward.z).normalize_or_zero();
    let ext_drop = ext_feet + ext_flat * 0.8;
    println!(
        "exterior arrival eye: [{:.2}, {:.2}, {:.2}]",
        ext_eye.x, ext_eye.y, ext_eye.z
    );
    println!(
        "exterior predicted drop point: [{:.2}, {:.2}, {:.2}]",
        ext_drop.x, ext_drop.y, ext_drop.z
    );
    println!(
        "exterior lowered --camera: --camera {:.2},{:.2},{:.2}:{:.2},{:.2},{:.2}",
        ext_eye.x, ext_eye.y, ext_eye.z, ext_drop.x, ext_drop.y, ext_drop.z
    );
}
