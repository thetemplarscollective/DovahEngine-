//! Decode probe for the bhk parser: run `collision_shapes()` over
//! real Riverwood NIFs and dump what comes out — shape kinds,
//! layers, materials, vertex/triangle counts, AABBs, and (for
//! meshes) the per-material triangle normal stats that identify
//! stair ramps vs treads.
//!
//! Usage: cargo run -p dovah-app --example collision_decode --
//!        /tmp/collnifs/walkwaystairs1.nif [...]

use dovah_nif::{CollisionShape, NifFile};
use std::env;

fn aabb(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for p in points {
        for i in 0..3 {
            mn[i] = mn[i].min(p[i]);
            mx[i] = mx[i].max(p[i]);
        }
    }
    (mn, mx)
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    for path in &args {
        println!("=== {path} ===");
        let nif = match NifFile::open(path) {
            Ok(n) => n,
            Err(e) => {
                println!("  parse failed: {e}");
                continue;
            }
        };
        let bhk: Vec<&str> = nif
            .blocks
            .iter()
            .map(|b| b.type_name())
            .filter(|t| t.starts_with("bhk"))
            .collect();
        println!("  blocks: {}, bhk blocks: {}", nif.blocks.len(), bhk.len());
        // Parsed vs Unknown bhk blocks (a parse failure degrades
        // to Unknown — those still report their type name, so
        // count them separately by variant).
        let mut parsed = 0usize;
        let mut unknown = 0usize;
        for b in &nif.blocks {
            let t = b.type_name();
            if !t.starts_with("bhk") {
                continue;
            }
            if matches!(b, dovah_nif::Block::Unknown { .. }) {
                unknown += 1;
            } else {
                parsed += 1;
            }
        }
        println!("  bhk parsed: {parsed}, bhk unknown: {unknown}");
        let shapes = nif.collision_shapes();
        println!("  collision shapes: {}", shapes.len());
        for (i, shape) in shapes.iter().enumerate() {
            match shape {
                CollisionShape::Mesh {
                    vertices,
                    triangles,
                    material,
                    layer,
                } => {
                    let (mn, mx) = aabb(vertices);
                    // Normal stats: mean up-component + fraction
                    // steep (nz < 0.5) — NIF space is Z-up, so
                    // the up component is nz. Ramps are tilted,
                    // treads are flat (nz ~ 1), risers are
                    // vertical (nz ~ 0).
                    let mut nz_sum = 0.0f32;
                    let mut steep = 0usize;
                    let mut flat = 0usize;
                    for t in triangles {
                        let (a, b, c) = (
                            vertices[t[0] as usize],
                            vertices[t[1] as usize],
                            vertices[t[2] as usize],
                        );
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
                        if len < 1e-9 {
                            continue;
                        }
                        let unz = nz / len;
                        nz_sum += unz;
                        if unz < 0.5 {
                            steep += 1;
                        } else {
                            flat += 1;
                        }
                    }
                    let n = triangles.len().max(1) as f32;
                    println!(
                        "  [{i}] Mesh mat=0x{material:08x} layer={layer} verts={} tris={} AABB=({:.0},{:.0},{:.0})..({:.0},{:.0},{:.0})u mean_nz={:.2} flat={} steep={}",
                        vertices.len(),
                        triangles.len(),
                        mn[0],
                        mn[1],
                        mn[2],
                        mx[0],
                        mx[1],
                        mx[2],
                        nz_sum / n,
                        flat,
                        steep,
                    );
                }
                CollisionShape::Capsule {
                    a,
                    b,
                    radius,
                    layer,
                } => {
                    println!(
                        "  [{i}] Capsule layer={layer} a=({:.0},{:.0},{:.0}) b=({:.0},{:.0},{:.0}) r={:.0}u",
                        a[0], a[1], a[2], b[0], b[1], b[2], radius
                    );
                }
                CollisionShape::Hull { points, layer } => {
                    let (mn, mx) = aabb(points);
                    println!(
                        "  [{i}] Hull layer={layer} pts={} AABB=({:.0},{:.0},{:.0})..({:.0},{:.0},{:.0})u",
                        points.len(),
                        mn[0],
                        mn[1],
                        mn[2],
                        mx[0],
                        mx[1],
                        mx[2],
                    );
                }
            }
        }
        // Visual meshes, for the AABB cross-check (collision
        // should sit on the visuals).
        let meshes = nif.meshes();
        let mut vmn = [f32::INFINITY; 3];
        let mut vmx = [f32::NEG_INFINITY; 3];
        for m in &meshes {
            for v in &m.vertices {
                // Apply the composed transform (meshes() keeps
                // vertices local).
                let t = &m.transform;
                let r = &t.rotation;
                let s = t.scale;
                let x = (r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2]) * s + t.translation[0];
                let y = (r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2]) * s + t.translation[1];
                let z = (r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2]) * s + t.translation[2];
                vmn[0] = vmn[0].min(x);
                vmn[1] = vmn[1].min(y);
                vmn[2] = vmn[2].min(z);
                vmx[0] = vmx[0].max(x);
                vmx[1] = vmx[1].max(y);
                vmx[2] = vmx[2].max(z);
            }
        }
        println!(
            "  visual meshes: {} AABB=({:.0},{:.0},{:.0})..({:.0},{:.0},{:.0})u",
            meshes.len(),
            vmn[0],
            vmn[1],
            vmn[2],
            vmx[0],
            vmx[1],
            vmx[2],
        );
    }
}
