//! Visual-mesh triangle dump for one NIF: every triangle's
//! centroid + normal z, grouped by mesh — to see the actual
//! shape (stair profile) that the collision approximates.
//!
//! Usage: cargo run -p dovah-app --example visual_tris -- <nif>

use dovah_nif::NifFile;
use std::env;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let nif = NifFile::open(&args[0]).expect("parse NIF");
    for (mi, m) in nif.meshes().iter().enumerate() {
        let t = &m.transform;
        println!(
            "--- mesh {mi} ({} verts, {} tris) t=({:.0},{:.0},{:.0}) scale={:.2} ---",
            m.vertices.len(),
            m.indices.len() / 3,
            t.translation[0],
            t.translation[1],
            t.translation[2],
            t.scale,
        );
        let mut tris: Vec<(f32, f32, f32, f32)> = Vec::new();
        for tri in m.indices.as_chunks::<3>().0 {
            // Apply the composed transform (row-major Mat3).
            let world = |v: [f32; 3]| -> [f32; 3] {
                let r = &t.rotation;
                let s = t.scale;
                [
                    (r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2]) * s + t.translation[0],
                    (r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2]) * s + t.translation[1],
                    (r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2]) * s + t.translation[2],
                ]
            };
            let (a, b, c) = (
                world(m.vertices[tri[0] as usize]),
                world(m.vertices[tri[1] as usize]),
                world(m.vertices[tri[2] as usize]),
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
            let nz = ux * vy - uy * vx;
            let nx = uy * vz - uz * vy;
            let ny = uz * vx - ux * vz;
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            let unz = if len < 1e-9 { 0.0 } else { nz / len };
            tris.push((cx, cy, cz, unz));
        }
        tris.sort_by(|a, b| {
            a.2.partial_cmp(&b.2)
                .unwrap()
                .then(a.1.partial_cmp(&b.1).unwrap())
        });
        // Print a compact profile: for each distinct z band,
        // the y range and the nz values. Full dump if < 120 tris.
        if tris.len() <= 120 {
            for (_cx, cy, cz, nz) in &tris {
                println!("  c=({cx:7.1},{cy:7.1},{cz:7.1}) nz={nz:+.2}");
            }
        } else {
            // Band summary: group by rounded z.
            let mut bands: Vec<(i32, f32, f32, f32, f32, usize)> = Vec::new();
            for (cx, cy, cz, nz) in &tris {
                let z = cz.round() as i32;
                if let Some(b) = bands.iter_mut().find(|b| b.0 == z) {
                    b.1 = b.1.min(*cy);
                    b.2 = b.2.max(*cy);
                    b.3 = b.3.min(*nz);
                    b.4 = b.4.max(*nz);
                    b.5 += 1;
                } else {
                    bands.push((z, *cy, *cy, *nz, *nz, 1));
                }
            }
            bands.sort_by_key(|b| b.0);
            for (z, ymin, ymax, nzmin, nzmax, n) in bands {
                println!("  z={z:6} y {ymin:7.1}..{ymax:7.1} nz {nzmin:+.2}..{nzmax:+.2} n={n}");
            }
        }
    }
}
