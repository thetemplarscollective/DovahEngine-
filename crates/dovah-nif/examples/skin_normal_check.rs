//! Check reconstructed skinned-mesh normals: for each skinned
//! mesh, report whether it carries normals and what fraction of
//! them point away from the mesh centroid (outward). A mesh whose
//! normals point inward renders dark — the M12 dark-hands check.
//! Usage: `cargo run -p dovah-nif --example skin_normal_check -- <nif-file>...`

use dovah_nif::NifFile;
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let paths: Vec<String> = env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: skin_normal_check <nif-file>...");
        return ExitCode::from(2);
    }
    for path in &paths {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{path}: cannot read: {e}");
                continue;
            }
        };
        let nif = match NifFile::from_bytes(&bytes) {
            Ok(n) => n,
            Err(e) => {
                eprintln!("{path}: parse failed: {e:?}");
                continue;
            }
        };
        println!("{path}:");
        for mesh in nif.meshes() {
            if !mesh.skinned {
                continue;
            }
            // UV bounds: where on the texture this mesh samples.
            if !mesh.uvs.is_empty() {
                let (mut umin, mut umax, mut vmin, mut vmax) =
                    (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
                for uv in &mesh.uvs {
                    umin = umin.min(uv[0]);
                    umax = umax.max(uv[0]);
                    vmin = vmin.min(uv[1]);
                    vmax = vmax.max(uv[1]);
                }
                println!(
                    "  mesh {:?}: UV u [{umin:.3}, {umax:.3}] v [{vmin:.3}, {vmax:.3}]",
                    mesh.name
                );
            } else {
                println!("  mesh {:?}: NO UVS", mesh.name);
            }
            let n = mesh.vertices.len() as f32;
            let centroid = mesh.vertices.iter().fold([0.0f32; 3], |acc, v| {
                [acc[0] + v[0] / n, acc[1] + v[1] / n, acc[2] + v[2] / n]
            });
            // Geometric winding check: for each triangle, does its
            // face normal (cross product, i.e. what smooth normals
            // derive from) point away from the centroid?
            let mut tri_out = 0usize;
            let mut tri_in = 0usize;
            for tri in mesh.indices.as_chunks::<3>().0 {
                let (a, b, c) = (
                    mesh.vertices[tri[0] as usize],
                    mesh.vertices[tri[1] as usize],
                    mesh.vertices[tri[2] as usize],
                );
                let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let n = [
                    ab[1] * ac[2] - ab[2] * ac[1],
                    ab[2] * ac[0] - ab[0] * ac[2],
                    ab[0] * ac[1] - ab[1] * ac[0],
                ];
                let mid = [
                    (a[0] + b[0] + c[0]) / 3.0 - centroid[0],
                    (a[1] + b[1] + c[1]) / 3.0 - centroid[1],
                    (a[2] + b[2] + c[2]) / 3.0 - centroid[2],
                ];
                if n[0] * mid[0] + n[1] * mid[1] + n[2] * mid[2] > 0.0 {
                    tri_out += 1;
                } else {
                    tri_in += 1;
                }
            }
            let tri_total = tri_out + tri_in;
            println!(
                "  mesh {:?}: triangles facing outward {tri_out}/{tri_total} ({:.0}%)",
                mesh.name,
                tri_out as f32 / tri_total as f32 * 100.0
            );
            // Signed volume (divergence theorem): positive for a
            // closed mesh with outward CCW winding, negative when
            // the winding is reversed. Hand vs body decides whether
            // the hand's triangles wind the other way.
            let mut volume = 0.0f64;
            for tri in mesh.indices.as_chunks::<3>().0 {
                let (a, b, c) = (
                    mesh.vertices[tri[0] as usize],
                    mesh.vertices[tri[1] as usize],
                    mesh.vertices[tri[2] as usize],
                );
                let cross = [
                    (b[1] as f64 * c[2] as f64 - b[2] as f64 * c[1] as f64),
                    (b[2] as f64 * c[0] as f64 - b[0] as f64 * c[2] as f64),
                    (b[0] as f64 * c[1] as f64 - b[1] as f64 * c[0] as f64),
                ];
                volume +=
                    (a[0] as f64 * cross[0] + a[1] as f64 * cross[1] + a[2] as f64 * cross[2])
                        / 6.0;
            }
            println!("  mesh {:?}: signed volume {volume:.1}", mesh.name);
            if mesh.normals.is_empty() {
                println!(
                    "  mesh {:?}: NO NORMALS ({} verts) — renderer computes smooth normals from winding",
                    mesh.name,
                    mesh.vertices.len()
                );
                continue;
            }
            let mut outward = 0usize;
            let mut inward = 0usize;
            for (v, normal) in mesh.vertices.iter().zip(mesh.normals.iter()) {
                let d = [v[0] - centroid[0], v[1] - centroid[1], v[2] - centroid[2]];
                let dot = d[0] * normal[0] + d[1] * normal[1] + d[2] * normal[2];
                if dot > 0.0 {
                    outward += 1;
                } else {
                    inward += 1;
                }
            }
            let total = outward + inward;
            println!(
                "  mesh {:?}: normals present, outward {outward}/{total} ({:.0}%)",
                mesh.name,
                outward as f32 / total as f32 * 100.0
            );
        }
    }
    ExitCode::SUCCESS
}
