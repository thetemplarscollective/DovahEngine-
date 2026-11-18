//! Per-shape UV range + texture + bounds for NIFs: does a
//! shape's texture tile (UV range > 1)? With Bevy's default
//! ClampToEdge sampler a tiled UV smears the edge pixels into
//! a flat colour instead of repeating the texture.
//!
//! Floor investigation (Dylan 02:08): the inn floor renders as
//! a flat tan plane — check the FarmInt shell NIFs.
//!
//! Usage: cargo run -p dovah-nif --example uv_probe --
//!   <nif-path-in-meshes-bsa> [nif-path...]

use dovah_bsa::BsaArchive;
use dovah_nif::NifFile;
use std::env;

const MESHES0: &str = "test-data/Skyrim_-_Meshes0.bsa";
const MESHES1: &str = "test-data/Skyrim_-_Meshes1.bsa";

fn main() {
    let paths: Vec<String> = env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: uv_probe <nif-path> [nif-path...]");
        return;
    }
    let mut bsa0 = BsaArchive::open(MESHES0).expect("open Meshes0");
    let mut bsa1 = BsaArchive::open(MESHES1).expect("open Meshes1");
    for path in &paths {
        let norm = path.replace('\\', "/").to_lowercase();
        let bytes = bsa0
            .extract(&norm)
            .or_else(|_| bsa1.extract(&norm))
            .unwrap_or_else(|_| panic!("{path} not found in Meshes BSAs"));
        let nif = NifFile::from_bytes(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        println!("NIF: {path}");
        for mesh in nif.meshes() {
            let (mut umin, mut umax) = (f32::MAX, f32::MIN);
            let (mut vmin, mut vmax) = (f32::MAX, f32::MIN);
            for uv in &mesh.uvs {
                umin = umin.min(uv[0]);
                umax = umax.max(uv[0]);
                vmin = vmin.min(uv[1]);
                vmax = vmax.max(uv[1]);
            }
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in &mesh.vertices {
                for i in 0..3 {
                    lo[i] = lo[i].min(v[i]);
                    hi[i] = hi[i].max(v[i]);
                }
            }
            let diffuse = mesh.textures.diffuse.as_deref().unwrap_or("(none)");
            println!(
                "  {:28} verts={:5} U[{:6.2},{:6.2}] V[{:6.2},{:6.2}] AABB x[{:7.1},{:7.1}] y[{:7.1},{:7.1}] z[{:7.1},{:7.1}] tex={}",
                mesh.name,
                mesh.vertices.len(),
                umin,
                umax,
                vmin,
                vmax,
                lo[0],
                hi[0],
                lo[1],
                hi[1],
                lo[2],
                hi[2],
                diffuse
            );
        }
    }
}
