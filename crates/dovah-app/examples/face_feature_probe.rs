//! Face-feature burial probe v2 (v0.4.13 defect 6): are the FaceGen
//! feature meshes (eyes, mouth, brows, beard) sitting ON the head
//! skin surface, or buried INSIDE it?
//!
//! v1 compared each feature vertex's Y against the skin's max Y
//! in an (x, z) neighborhood — broken for a closed head (the
//! nose/brow are "in front of" everything, so even the visibly
//! correct hair read 100% buried). v2 is a proper point-in-mesh
//! test: from each feature vertex, cast a ray forward (+Y, the
//! face-forward axis, slightly skewed to dodge shared edges) and
//! count skin-triangle crossings — odd = inside the skin.
//!
//! Usage: cargo run -p dovah-app --example face_feature_probe --
//!        <facegen-nif-path-in-meshes-bsa>

use dovah_bsa::BsaArchive;
use dovah_nif::NifFile;

const MESHES0: &str = "test-data/Skyrim_-_Meshes0.bsa";
const MESHES1: &str = "test-data/Skyrim_-_Meshes1.bsa";

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Möller–Trumbore: distance along `dir` from `orig` to the
/// triangle, or None. `dir` need not be normalized.
fn ray_tri(orig: [f32; 3], dir: [f32; 3], v0: [f32; 3], v1: [f32; 3], v2: [f32; 3]) -> Option<f32> {
    let e1 = sub(v1, v0);
    let e2 = sub(v2, v0);
    let p = cross(dir, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let t = sub(orig, v0);
    let u = dot(t, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(t, e1);
    let v = dot(dir, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let dist = dot(e2, q) * inv;
    if dist > 1e-4 {
        Some(dist)
    } else {
        None
    }
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: face_feature_probe <facegen-nif-path>");
        return;
    }
    let mut bsa0 = BsaArchive::open(MESHES0).expect("open Meshes0");
    let mut bsa1 = BsaArchive::open(MESHES1).expect("open Meshes1");
    // Face-forward, skewed to dodge shared triangle edges.
    let dir = [0.13f32, 1.0, 0.29];
    for path in &paths {
        let norm = path.replace('\\', "/").to_lowercase();
        let bytes = bsa0
            .extract(&norm)
            .or_else(|_| bsa1.extract(&norm))
            .unwrap_or_else(|_| panic!("{path} not found in Meshes BSAs"));
        let nif = NifFile::from_bytes(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        println!("NIF: {path}");
        let meshes = nif.meshes();
        let skin = meshes
            .iter()
            .find(|m| {
                (m.name.contains("HeadBreton") || m.name.contains("HeadNord"))
                    && !m.name.contains("Hair")
            })
            .expect("head skin mesh");
        let st = skin.transform.translation;
        let skin_verts: Vec<[f32; 3]> = skin
            .vertices
            .iter()
            .map(|v| [v[0] + st[0], v[1] + st[1], v[2] + st[2]])
            .collect();
        let tris = skin.indices.len() / 3;
        println!(
            "  skin {:?}: {} verts, {} tris",
            skin.name,
            skin_verts.len(),
            tris
        );
        for mesh in &meshes {
            if std::ptr::eq(mesh, skin) {
                continue;
            }
            let t = mesh.transform.translation;
            let (mut inside, mut outside) = (0usize, 0usize);
            for v in &mesh.vertices {
                let o = [v[0] + t[0], v[1] + t[1], v[2] + t[2]];
                let mut hits = 0usize;
                for i in 0..tris {
                    let (a, b, c) = (
                        skin_verts[skin.indices[i * 3] as usize],
                        skin_verts[skin.indices[i * 3 + 1] as usize],
                        skin_verts[skin.indices[i * 3 + 2] as usize],
                    );
                    if ray_tri(o, dir, a, b, c).is_some() {
                        hits += 1;
                    }
                }
                if hits % 2 == 1 {
                    inside += 1;
                } else {
                    outside += 1;
                }
            }
            let total = inside + outside;
            println!(
                "  feature {:?}: {} verts, INSIDE={} ({:.0}%) OUTSIDE={} ({:.0}%)",
                mesh.name,
                total,
                inside,
                inside as f32 / total as f32 * 100.0,
                outside,
                outside as f32 / total as f32 * 100.0,
            );
        }
    }
}
