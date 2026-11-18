//! West-wall check: which inn refs sit near (-22, -9.50)
//! (where the collision_walk -X walk reached the bounds edge
//! without blocking), and what collision do they carry?
//!
//! Usage: cargo run -p dovah-app --example collision_westwall
//! (run from the workspace root; reads test-data/)

use dovah_app::assets::AssetProvider;
use dovah_app::cell::load_cell;
use dovah_app::collision::layer_blocks_movement;
use dovah_app::coords;
use dovah_esm::Plugin;
use dovah_nif::NifFile;
use std::path::Path;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const INN_EDID: &str = "RiverwoodSleepingGiantInn";
const POINT: (f32, f32) = (-22.0, -9.50);

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut provider = AssetProvider::new(Path::new("test-data"));
    let interior = load_cell(&plugin, INN_EDID, None).expect("load inn interior");
    println!("inn interior: {} objects", interior.objects.len());
    let mut near: Vec<(&dovah_app::cell::PlacedObject, f32)> = Vec::new();
    for object in &interior.objects {
        if object.is_actor || object.model_path.is_none() {
            continue;
        }
        let pos = coords::to_bevy_position(object.position);
        let d = ((pos.x - POINT.0).powi(2) + (pos.z - POINT.1).powi(2)).sqrt();
        if d < 6.0 {
            near.push((object, d));
        }
    }
    near.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    println!(
        "refs within 6 m of ({}, {}): {}",
        POINT.0,
        POINT.1,
        near.len()
    );
    for (object, d) in near {
        let pos = coords::to_bevy_position(object.position);
        let nif_path = format!("meshes\\{}", object.model_path.as_deref().unwrap());
        let shapes = provider
            .read_file(&nif_path)
            .and_then(|bytes| NifFile::from_bytes(&bytes).ok())
            .map(|nif| nif.collision_shapes())
            .unwrap_or_default();
        let blocking = shapes
            .iter()
            .filter(|s| layer_blocks_movement(s.layer()))
            .count();
        let layers: Vec<u8> = shapes.iter().map(|s| s.layer()).collect();
        println!(
            "  0x{:08x} {} ({}) at ({:.2}, {:.2}, {:.2}) d={d:.2} m: {} shapes, {} blocking, layers {layers:?}",
            object.ref_form_id,
            object.base_edid,
            object.base_type,
            pos.x,
            pos.y,
            pos.z,
            shapes.len(),
            blocking,
        );
    }
    // Also: the inn's room shells — do they carry collision at all?
    println!("\nroom shells (EDID contains InnEnd/InnMid/InnCorner):");
    let mut seen = std::collections::HashSet::new();
    for object in &interior.objects {
        if !object.base_edid.contains("InnEnd")
            && !object.base_edid.contains("InnMid")
            && !object.base_edid.contains("InnCorner")
        {
            continue;
        }
        let Some(model_path) = &object.model_path else {
            continue;
        };
        if !seen.insert(model_path.clone()) {
            continue;
        }
        let nif_path = format!("meshes\\{model_path}");
        let shapes = provider
            .read_file(&nif_path)
            .and_then(|bytes| NifFile::from_bytes(&bytes).ok())
            .map(|nif| nif.collision_shapes())
            .unwrap_or_default();
        let layers: Vec<u8> = shapes.iter().map(|s| s.layer()).collect();
        println!(
            "  {} ({}): {} shapes, layers {layers:?}",
            object.base_edid,
            model_path,
            shapes.len()
        );
    }
}
