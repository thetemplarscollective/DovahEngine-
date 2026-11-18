//! Dark-tint probe (inn lighting, Dylan's correction 2026-11-17):
//! the OBJECTS on the inn counter (bottle, mug, plate, barrel,
//! NPC clothes) render near-black while the counter wood beside
//! them reads fine — same light, same area, selective. Per shape,
//! report what could make it dark: vertex-color mean/min (a dark
//! multiplier crushes the texture), stored-normals-vs-winding
//! mean dot (inverted normals render ambient-only — the FaceGen
//! void pattern), and the diffuse/normal texture paths.
//!
//! Usage: cargo run -p dovah-nif --example tint_probe --
//!   <nif-path-in-meshes-bsa> [nif-path...]

use dovah_bsa::BsaArchive;
use dovah_nif::NifFile;
use std::env;

const MESHES0: &str = "test-data/Skyrim_-_Meshes0.bsa";
const MESHES1: &str = "test-data/Skyrim_-_Meshes1.bsa";

/// Mean dot of the winding face normal vs the outward
/// direction (triangle centroid minus mesh centroid), sampled
/// like `normal_dot`. ~+1.0 = the winding faces outward (CCW
/// as seen from outside, u×v is the true outward normal);
/// ~-1.0 = the winding faces inward (CW — u×v points into the
/// mesh, so a negative stored-normal dot means the stored
/// normals are CORRECT and the engine's flip breaks them).
fn winding_outward(vertices: &[[f32; 3]], indices: &[u32]) -> Option<f32> {
    let tri_count = indices.len() / 3;
    if tri_count == 0 || vertices.is_empty() {
        return None;
    }
    // Mesh centroid.
    let mut c = [0.0f32; 3];
    for v in vertices {
        c[0] += v[0];
        c[1] += v[1];
        c[2] += v[2];
    }
    let n = vertices.len() as f32;
    c = [c[0] / n, c[1] / n, c[2] / n];
    let stride = (tri_count / 64).max(1);
    let mut sum = 0.0f32;
    let mut count = 0usize;
    for t in (0..tri_count).step_by(stride) {
        let (ia, ib, ic) = (
            indices[t * 3] as usize,
            indices[t * 3 + 1] as usize,
            indices[t * 3 + 2] as usize,
        );
        if ia >= vertices.len() || ib >= vertices.len() || ic >= vertices.len() {
            continue;
        }
        let (a, b, cc) = (vertices[ia], vertices[ib], vertices[ic]);
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [cc[0] - a[0], cc[1] - a[1], cc[2] - a[2]];
        let mut f = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let fl = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
        if fl < 1e-9 {
            continue;
        }
        f = [f[0] / fl, f[1] / fl, f[2] / fl];
        // Triangle centroid minus mesh centroid, normalized.
        let mut o = [
            (a[0] + b[0] + cc[0]) / 3.0 - c[0],
            (a[1] + b[1] + cc[1]) / 3.0 - c[1],
            (a[2] + b[2] + cc[2]) / 3.0 - c[2],
        ];
        let ol = (o[0] * o[0] + o[1] * o[1] + o[2] * o[2]).sqrt();
        if ol < 1e-9 {
            continue;
        }
        o = [o[0] / ol, o[1] / ol, o[2] / ol];
        sum += f[0] * o[0] + f[1] * o[1] + f[2] * o[2];
        count += 1;
    }
    if count >= 4 {
        Some(sum / count as f32)
    } else {
        None
    }
}

/// Mean dot of stored vertex normals vs the winding face normal,
/// sampled like `stored_normals_are_inverted` in dovah-app's
/// convert.rs (up to 64 triangles). ~+1.0 = normals agree with
/// the winding (correct); ~-1.0 = inverted (renders dark);
/// the engine flips at <= -0.3.
fn normal_dot(vertices: &[[f32; 3]], indices: &[u32], normals: &[[f32; 3]]) -> Option<f32> {
    let tri_count = indices.len() / 3;
    if tri_count == 0 || normals.is_empty() {
        return None;
    }
    let stride = (tri_count / 64).max(1);
    let mut sum = 0.0f32;
    let mut n = 0usize;
    for t in (0..tri_count).step_by(stride) {
        let (ia, ib, ic) = (
            indices[t * 3] as usize,
            indices[t * 3 + 1] as usize,
            indices[t * 3 + 2] as usize,
        );
        if ia >= vertices.len()
            || ib >= vertices.len()
            || ic >= vertices.len()
            || ia >= normals.len()
            || ib >= normals.len()
            || ic >= normals.len()
        {
            continue;
        }
        let (a, b, c) = (vertices[ia], vertices[ib], vertices[ic]);
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let mut f = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let fl = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
        if fl < 1e-9 {
            continue;
        }
        f = [f[0] / fl, f[1] / fl, f[2] / fl];
        let mut s = [
            normals[ia][0] + normals[ib][0] + normals[ic][0],
            normals[ia][1] + normals[ib][1] + normals[ic][1],
            normals[ia][2] + normals[ib][2] + normals[ic][2],
        ];
        let sl = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
        if sl < 1e-6 {
            continue;
        }
        s = [s[0] / sl, s[1] / sl, s[2] / sl];
        sum += f[0] * s[0] + f[1] * s[1] + f[2] * s[2];
        n += 1;
    }
    if n >= 4 {
        Some(sum / n as f32)
    } else {
        None
    }
}

fn main() {
    let paths: Vec<String> = env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: tint_probe <nif-path> [nif-path...]");
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
            // Vertex colors.
            let vc = if mesh.vertex_colors.is_empty() {
                "none (white)".to_string()
            } else {
                let n = mesh.vertex_colors.len() as f32;
                let (mut r, mut g, mut b) = (0f32, 0f32, 0f32);
                let (mut rmin, mut gmin, mut bmin) = (1f32, 1f32, 1f32);
                for c in &mesh.vertex_colors {
                    r += c[0];
                    g += c[1];
                    b += c[2];
                    rmin = rmin.min(c[0]);
                    gmin = gmin.min(c[1]);
                    bmin = bmin.min(c[2]);
                }
                format!(
                    "mean ({:.3},{:.3},{:.3}) min ({:.3},{:.3},{:.3})",
                    r / n,
                    g / n,
                    b / n,
                    rmin,
                    gmin,
                    bmin
                )
            };
            // Stored normals vs winding.
            let dot = normal_dot(&mesh.vertices, &mesh.indices, &mesh.normals)
                .map(|d| format!("{d:+.3}"))
                .unwrap_or_else(|| "n/a".to_string());
            // Winding vs outward direction (is u×v the true normal?).
            let wo = winding_outward(&mesh.vertices, &mesh.indices)
                .map(|d| format!("{d:+.3}"))
                .unwrap_or_else(|| "n/a".to_string());
            println!(
                "  shape {:?}: verts={} vcolors={} normal_dot={} winding_out={} diffuse={:?} normal_map={:?} unlit={} emissive={:?}",
                mesh.name,
                mesh.vertices.len(),
                vc,
                dot,
                wo,
                mesh.textures.diffuse,
                mesh.textures.normal,
                mesh.unlit,
                mesh.emissive,
            );
        }
    }
}
