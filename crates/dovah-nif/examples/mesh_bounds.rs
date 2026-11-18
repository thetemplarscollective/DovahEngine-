//! One-off probe: for a NIF in the Meshes BSAs, print each
//! mesh's name, local AABB (Skyrim units), blend mode, and
//! diffuse texture — to identify which effect mesh makes the
//! grey veil over the Riverwood mid-ground.
//!
//! Usage: cargo run -q -p dovah-nif --example mesh_bounds --
//!        <nif-path> [<nif-path> ...]
//! e.g. meshes/effects/fxwaterfallthin2048x128.nif

use dovah_bsa::BsaArchive;
use dovah_nif::NifFile;
use std::env;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    assert!(!args.is_empty(), "usage: mesh_bounds <nif-path> ...");
    let mut archives = Vec::new();
    for name in [
        "test-data/Skyrim_-_Meshes0.bsa",
        "test-data/Skyrim_-_Meshes1.bsa",
    ] {
        if let Ok(a) = BsaArchive::open(name) {
            archives.push(a);
        }
    }
    for want in &args {
        let lower = want.replace('\\', "/").to_lowercase();
        let mut bytes = None;
        for archive in archives.iter_mut() {
            let names: Vec<String> = archive.list().into_iter().map(str::to_string).collect();
            if let Some(stored) = names.iter().find(|s| s.to_lowercase() == lower) {
                bytes = archive.extract(stored).ok();
                break;
            }
        }
        let Some(bytes) = bytes else {
            println!("{want}: NOT FOUND");
            continue;
        };
        let nif = NifFile::from_bytes(&bytes).expect("parse NIF");
        println!("== {want} ==");
        for mesh in nif.meshes() {
            let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in &mesh.vertices {
                for i in 0..3 {
                    mn[i] = mn[i].min(v[i]);
                    mx[i] = mx[i].max(v[i]);
                }
            }
            println!(
                "  '{}' {} verts AABB min=({:.0},{:.0},{:.0}) max=({:.0},{:.0},{:.0}) size=({:.0}x{:.0}x{:.0}) blend={:?} unlit={} diffuse={:?}",
                mesh.name,
                mesh.vertices.len(),
                mn[0],
                mn[1],
                mn[2],
                mx[0],
                mx[1],
                mx[2],
                mx[0] - mn[0],
                mx[1] - mn[1],
                mx[2] - mn[2],
                mesh.alpha_blend,
                mesh.unlit,
                mesh.textures.diffuse,
            );
        }
    }
}
