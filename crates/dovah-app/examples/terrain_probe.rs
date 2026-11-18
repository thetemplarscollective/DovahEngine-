//! One-off probe: dump the baked terrain texture for every
//! cell of the Riverwood 3x3 block and print per-cell stats
//! (mean/std + base texture + layer count). The hillside
//! renders as a flat grey sheet while the road reads as dark
//! textured dirt — this decides whether the BAKE is uniform
//! (VTXT/base problem) or the bake varies and the problem is
//! downstream (material/UVs).
//!
//! Usage: cargo run -q -p dovah-app --example terrain_probe
//! Output: /tmp/terrain_bake_<gx>_<gy>.ppm per cell

use dovah_app::assets::AssetProvider;
use dovah_app::cell::load_exterior;
use dovah_app::vtxt::{bake_terrain_texture, DecodedTexture};
use dovah_esm::Plugin;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn stats(data: &[u8]) -> (f64, f64) {
    let n = (data.len() / 4) as f64;
    let mut sum = 0f64;
    let mut sum2 = 0f64;
    for p in data.as_chunks::<4>().0 {
        // Luminance of the RGB texel.
        let l = 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
        sum += l;
        sum2 += l * l;
    }
    let mean = sum / n;
    let std = (sum2 / n - mean * mean).max(0.0).sqrt();
    (mean, std)
}

fn main() {
    let esm = PathBuf::from("test-data/Skyrim.esm");
    let plugin = Plugin::open(&esm).expect("open ESM");
    let cell = load_exterior(&plugin, (5, -11), None).expect("load exterior");
    println!("{} land cells", cell.lands.len());
    let mut assets = AssetProvider::new(&PathBuf::from("test-data"));
    let mut cache: HashMap<String, Option<Arc<DecodedTexture>>> = HashMap::new();
    for land in &cell.lands {
        let (gx, gy) = land.grid;
        let base = land.base_diffuse.clone().unwrap_or_default();
        let layers = land.layers.len();
        // Mean opacity across all layers (0 = no VTXT cover).
        let mean_op: f64 = if layers == 0 {
            0.0
        } else {
            land.layers
                .iter()
                .flat_map(|l| l.opacity.iter())
                .map(|o| *o as f64)
                .sum::<f64>()
                / (layers * 17 * 17) as f64
        };
        match bake_terrain_texture(land, &mut assets, &mut cache) {
            Some(image) => {
                let data = image.data.as_ref().unwrap();
                let (mean, std) = stats(data);
                println!(
                    "cell ({gx},{gy}): base={base} layers={layers} mean_opacity={mean_op:.3} bake {}x{} mean_lum={mean:.1} std={std:.1}",
                    image.width(),
                    image.height()
                );
                let w = image.width() as usize;
                let h = image.height() as usize;
                let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
                for p in data.as_chunks::<4>().0 {
                    ppm.extend_from_slice(&p[..3]);
                }
                let out = format!("/tmp/terrain_bake_{gx}_{gy}.ppm");
                std::fs::write(&out, ppm).unwrap();
                println!("  wrote {out}");
            }
            None => println!(
                "cell ({gx},{gy}): base={base} layers={layers} mean_opacity={mean_op:.3} BAKE FAILED"
            ),
        }
    }
}
