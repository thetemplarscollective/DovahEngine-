//! Per-shape vertex color + bounds stats for one NIF file: mean /
//! min / max RGBA of the vertex colors and the local AABB.
//!
//! M13: the full pine foliage renders as thin black twigs while the
//! LOD foliage rendered green — is the full foliage's vertex alpha
//! (or RGB) the difference? And how big are the waterfall skirt's
//! fog sheets, to settle whether they are the white band?
//!
//! Usage: cargo run -p dovah-nif --example vcolor_stats --
//!   <nif-path-in-meshes-bsa> [nif-path...]

use dovah_bsa::BsaArchive;
use dovah_nif::NifFile;
use std::env;

const MESHES0: &str = "test-data/Skyrim_-_Meshes0.bsa";
const MESHES1: &str = "test-data/Skyrim_-_Meshes1.bsa";

fn main() {
    let paths: Vec<String> = env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: vcolor_stats <nif-path> [nif-path...]");
        return;
    }
    let mut bsa0 = BsaArchive::open(MESHES0).expect("open Meshes0");
    let mut bsa1 = BsaArchive::open(MESHES1).expect("open Meshes1");
    for path in &paths {
        // BSA paths are stored lowercase with forward slashes
        // (same normalization as the app's asset provider).
        let norm = path.replace('\\', "/").to_lowercase();
        let bytes = bsa0
            .extract(&norm)
            .or_else(|_| bsa1.extract(&norm))
            .unwrap_or_else(|_| panic!("{path} not found in Meshes BSAs"));
        let nif = NifFile::from_bytes(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        println!("NIF: {path}");
        for mesh in nif.meshes() {
            let n = mesh.vertices.len() as f32;
            let (mut r, mut g, mut b, mut a) = (0f32, 0f32, 0f32, 0f32);
            let (mut rmin, mut gmin, mut bmin, mut amin) = (1f32, 1f32, 1f32, 1f32);
            let (mut rmax, mut gmax, mut bmax, mut amax) = (0f32, 0f32, 0f32, 0f32);
            let mut has_color = false;
            if !mesh.vertex_colors.is_empty() {
                has_color = true;
                for c in &mesh.vertex_colors {
                    r += c[0];
                    g += c[1];
                    b += c[2];
                    a += c[3];
                    rmin = rmin.min(c[0]);
                    gmin = gmin.min(c[1]);
                    bmin = bmin.min(c[2]);
                    amin = amin.min(c[3]);
                    rmax = rmax.max(c[0]);
                    gmax = gmax.max(c[1]);
                    bmax = bmax.max(c[2]);
                    amax = amax.max(c[3]);
                }
            }
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in &mesh.vertices {
                for i in 0..3 {
                    lo[i] = lo[i].min(v[i]);
                    hi[i] = hi[i].max(v[i]);
                }
            }
            if has_color {
                println!(
                    "  '{}' verts={} color mean=({:.2},{:.2},{:.2},{:.2}) min=({:.2},{:.2},{:.2},{:.2}) max=({:.2},{:.2},{:.2},{:.2})",
                    mesh.name,
                    mesh.vertices.len(),
                    r / n,
                    g / n,
                    b / n,
                    a / n,
                    rmin,
                    gmin,
                    bmin,
                    amin,
                    rmax,
                    gmax,
                    bmax,
                    amax
                );
            } else {
                println!(
                    "  '{}' verts={} no vertex colors",
                    mesh.name,
                    mesh.vertices.len()
                );
            }
            println!(
                "    local AABB min=({:.0},{:.0},{:.0}) max=({:.0},{:.0},{:.0}) size=({:.0},{:.0},{:.0})u",
                lo[0],
                lo[1],
                lo[2],
                hi[0],
                hi[1],
                hi[2],
                hi[0] - lo[0],
                hi[1] - lo[1],
                hi[2] - lo[2]
            );
        }
    }
}
