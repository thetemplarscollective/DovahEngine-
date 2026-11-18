//! Normal-map convention probe: are the NIF's stored tangents
//! the geometric tangents (T = dP/dU), or negated? Compares the
//! stored tangent/bitangent against tangents computed from the
//! mesh's own positions + UVs + indices (the standard
//! UV-gradient method). If the stored tangents are negated, the
//! normal map's R and G channels must both be flipped (or the
//! tangents negated) for the relief to read the right way up.
//!
//! Usage: cargo run -p dovah-nif --example tangent_check --
//!   <nif-path-in-meshes-bsa>
//! (default: the inn floor shell, FarmIntInnEnd01.)

use dovah_bsa::BsaArchive;
use dovah_nif::NifFile;

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-9);
    [a[0] / l, a[1] / l, a[2] / l]
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        "meshes/architecture/farmhouse/interior/farmintinnend01.nif".to_string()
    });
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("test-data");
    let mut bytes = None;
    for name in ["Skyrim_-_Meshes0.bsa", "Skyrim_-_Meshes1.bsa"] {
        let mut archive = BsaArchive::open(root.join(name)).expect("open BSA");
        if let Ok(b) = archive.extract(&path) {
            bytes = Some(b);
            break;
        }
    }
    let bytes = bytes.expect("NIF not found in Meshes BSAs");
    let nif = NifFile::from_bytes(&bytes).expect("parse NIF");
    let mut checked = 0usize;
    let mut t_pos = 0usize;
    let mut t_neg = 0usize;
    let mut b_pos = 0usize;
    let mut b_neg = 0usize;
    let mut mir_t_pos = 0usize;
    let mut mir_t_neg = 0usize;
    let mut mir_b_pos = 0usize;
    let mut mir_b_neg = 0usize;
    for mesh in nif.meshes() {
        if mesh.tangents.is_empty() || mesh.bitangents.is_empty() {
            continue;
        }
        // Geometric tangent from the first 64 triangles with
        // non-degenerate UVs.
        let mut tris = 0usize;
        for tri in mesh.indices.as_chunks::<3>().0 {
            if tris >= 64 {
                break;
            }
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            if i0 >= mesh.vertices.len() || i1 >= mesh.vertices.len() || i2 >= mesh.vertices.len() {
                continue;
            }
            let p0 = mesh.vertices[i0];
            let p1 = mesh.vertices[i1];
            let p2 = mesh.vertices[i2];
            let uv0 = mesh.uvs[i0];
            let uv1 = mesh.uvs[i1];
            let uv2 = mesh.uvs[i2];
            let e1 = sub(p1, p0);
            let e2 = sub(p2, p0);
            let du1 = uv1[0] - uv0[0];
            let dv1 = uv1[1] - uv0[1];
            let du2 = uv2[0] - uv0[0];
            let dv2 = uv2[1] - uv0[1];
            let det = du1 * dv2 - du2 * dv1;
            if det.abs() < 1e-9 {
                continue;
            }
            // Mirrored UV triangles (det < 0) flip the geometric
            // frame — the stored (smoothed, per-vertex) tangent
            // cannot match both, so split the counts by det
            // sign: standard tangents read +T/+B on det > 0 and
            // -T/-B on det < 0 (the handedness w carries the
            // mirror).
            let inv = 1.0 / det;
            let gt = norm([
                (dv2 * e1[0] - dv1 * e2[0]) * inv,
                (dv2 * e1[1] - dv1 * e2[1]) * inv,
                (dv2 * e1[2] - dv1 * e2[2]) * inv,
            ]);
            let gb = norm([
                (-du2 * e1[0] + du1 * e2[0]) * inv,
                (-du2 * e1[1] + du1 * e2[1]) * inv,
                (-du2 * e1[2] + du1 * e2[2]) * inv,
            ]);
            // Bevy never uses the stored bitangent: it uses the
            // tangent xyz + w, computing B = cross(N, T) * w.
            // So compare the geometric frame against what BEVY
            // builds: T_stored, and B_bevy = cross(N, T) * w
            // with w computed exactly as convert.rs does
            // (sign of (T x B) . N from the stored vectors).
            let st = norm(mesh.tangents[i0]);
            let sb = mesh.bitangents[i0];
            let sn = norm(mesh.normals[i0]);
            let cross_tb = [
                st[1] * sb[2] - st[2] * sb[1],
                st[2] * sb[0] - st[0] * sb[2],
                st[0] * sb[1] - st[1] * sb[0],
            ];
            let w = if dot(cross_tb, sn) >= 0.0 { 1.0 } else { -1.0 };
            let b_bevy = norm([
                w * (sn[1] * st[2] - sn[2] * st[1]),
                w * (sn[2] * st[0] - sn[0] * st[2]),
                w * (sn[0] * st[1] - sn[1] * st[0]),
            ]);
            let dt = dot(gt, st);
            let db = dot(gb, b_bevy);
            let (tp, tn, bp, bn) = if det > 0.0 {
                (&mut t_pos, &mut t_neg, &mut b_pos, &mut b_neg)
            } else {
                (
                    &mut mir_t_pos,
                    &mut mir_t_neg,
                    &mut mir_b_pos,
                    &mut mir_b_neg,
                )
            };
            if dt > 0.5 {
                *tp += 1;
            } else if dt < -0.5 {
                *tn += 1;
            }
            if db > 0.5 {
                *bp += 1;
            } else if db < -0.5 {
                *bn += 1;
            }
            checked += 1;
            tris += 1;
        }
        if checked > 0 {
            println!(
                "shape '{}': {} tris\n  det>0: T +{}/-{}, B +{}/-{}\n  det<0 (mirrored UVs): T +{}/-{}, B +{}/-{}",
                mesh.name, tris, t_pos, t_neg, b_pos, b_neg, mir_t_pos, mir_t_neg, mir_b_pos,
                mir_b_neg
            );
            break;
        }
    }
    if checked == 0 {
        println!("no shape with tangents + usable triangles found");
    }
}
