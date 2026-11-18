//! Triangle dump for one NIF's collision meshes: every
//! triangle's centroid, normal z (NIF space is Z-up), and
//! material — to see the actual stair geometry (ramp plane vs
//! treads vs risers).
//!
//! Usage: cargo run -p dovah-app --example collision_tris --
//!        <nif> [material-hex ...]
//! (no material filter = all triangles)

use dovah_nif::{CollisionShape, NifFile};
use std::env;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let path = &args[0];
    let mats: Vec<u32> = args[1..]
        .iter()
        .filter_map(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .collect();
    let nif = NifFile::open(path).expect("parse NIF");
    for shape in nif.collision_shapes() {
        let CollisionShape::Mesh {
            vertices,
            triangles,
            material,
            layer,
        } = shape
        else {
            continue;
        };
        if !mats.is_empty() && !mats.contains(&material) {
            continue;
        }
        println!(
            "--- material 0x{material:08x} layer {layer} ({} tris) ---",
            triangles.len()
        );
        let mut tris: Vec<(f32, f32, f32, f32, f32)> = Vec::new();
        for t in &triangles {
            let (a, b, c) = (
                vertices[t[0] as usize],
                vertices[t[1] as usize],
                vertices[t[2] as usize],
            );
            let cx = (a[0] + b[0] + c[0]) / 3.0;
            let cy = (a[1] + b[1] + c[1]) / 3.0;
            let cz = (a[2] + b[2] + c[2]) / 3.0;
            let ux = b[0] - a[0];
            let uy = b[1] - a[1];
            let uz = b[2] - a[2];
            let vx = c[0] - a[0];
            let vy = c[1] - a[1];
            let vz = c[2] - a[2];
            let nx = uy * vz - uz * vy;
            let ny = uz * vx - ux * vz;
            let nz = ux * vy - uy * vx;
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            let unz = if len < 1e-9 { 0.0 } else { nz / len };
            let unx = if len < 1e-9 { 0.0 } else { nx / len };
            tris.push((cx, cy, cz, unz, unx));
        }
        // Sort by height (z), then along the flight (y).
        tris.sort_by(|a, b| {
            a.2.partial_cmp(&b.2)
                .unwrap()
                .then(a.1.partial_cmp(&b.1).unwrap())
        });
        for (cx, cy, cz, nz, nx) in tris {
            println!("  c=({cx:7.1},{cy:7.1},{cz:7.1}) nz={nz:+.2} nx={nx:+.2}");
        }
    }
}
