//! One-off probe: does AssetProvider::read_file find the
//! fog gradient texture (the production asset path)? The
//! gradient multiply works on real data (multiply_probe) and
//! the fog diagnostic shows newFog faint (multiply applied),
//! but the exterior render is byte-identical to the
//! pre-gradient render — so either production never reads the
//! gradient bytes or the visible sheet isn't newFog.
//!
//! Usage: cargo run -q -p dovah-app --example gradient_probe

use dovah_app::assets::AssetProvider;
use std::path::PathBuf;

fn main() {
    let data = PathBuf::from("test-data");
    let mut provider = AssetProvider::new(&data);
    println!("indexed files: {}", provider.indexed_files());
    for path in [
        "textures\\effects\\FXFogHeavy.dds",
        "textures\\effects\\gradients\\GradCobWeb.dds",
        "textures/effects/gradients/gradcobweb.dds",
        "textures\\effects\\FXCloudRoundTile.dds",
    ] {
        match provider.read_file(path) {
            Some(bytes) => println!("{path}: FOUND {} bytes", bytes.len()),
            None => println!("{path}: MISSING"),
        }
    }
}
