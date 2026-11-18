//! Per-shape normals + glossiness probe (v0.4.13 rendering fixes).
//!
//! Two questions, one probe:
//! 1. DARK FACES: do the FaceGen head meshes carry normals at all?
//!    `tint_probe` reports normal_dot=n/a for the head skin — that
//!    means either empty normals (Bevy computes them from the
//!    winding: fine) or ZERO normals (no flip, no compute: the
//!    shape renders ambient-only = dark). This prints len, zero
//!    count and mean length per shape, which settles it.
//! 2. ROUGHNESS: what glossiness / specular values do Skyrim's
//!    `BSLightingShaderProperty` blocks actually carry? Bevy's
//!    default roughness 0.5 makes every surface semi-glossy — the
//!    prime suspect for the door glow, the white discs (specular
//!    hotspots) and the exterior grey wash (the specular ambient
//!    term dwarfs the diffuse ambient on dark albedos). The
//!    distribution decides between a flat roughness default and
//!    plumbing glossiness through.
//!
//! Usage: cargo run -p dovah-app --example normal_gloss_probe --
//!        <nif-path-in-meshes-bsa> [nif-path...]

use dovah_bsa::BsaArchive;
use dovah_nif::{Block, NifFile};

const MESHES0: &str = "test-data/Skyrim_-_Meshes0.bsa";
const MESHES1: &str = "test-data/Skyrim_-_Meshes1.bsa";

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: normal_gloss_probe <nif-path> [nif-path...]");
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
        println!("NIF: {path} ({} blocks)", nif.blocks.len());
        for mesh in nif.meshes() {
            let nn = mesh.normals.len();
            let (mut zeros, mut len_sum) = (0usize, 0f32);
            for n in &mesh.normals {
                let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                if l < 1e-6 {
                    zeros += 1;
                }
                len_sum += l;
            }
            let mean_len = if nn > 0 { len_sum / nn as f32 } else { 0.0 };
            println!(
                "  mesh {:?}: verts={} normals={} zero_normals={} mean_len={:.3} tangents={} uvs={} cutoff={:?} diffuse={:?}",
                mesh.name,
                mesh.vertices.len(),
                nn,
                zeros,
                mean_len,
                mesh.tangents.len(),
                mesh.uvs.len(),
                mesh.alpha_cutoff,
                mesh.textures.diffuse,
            );
        }
        for (i, b) in nif.blocks.iter().enumerate() {
            if let Block::LightingShaderProperty(p) = b {
                println!(
                    "  shader[{i}]: type={} gloss={:.1} spec_color=({:.2},{:.2},{:.2}) spec_strength={:.2} emissive=({:.2},{:.2},{:.2})x{:.2} flags1=0x{:08x}",
                    p.shader_type,
                    p.glossiness,
                    p.specular_color[0],
                    p.specular_color[1],
                    p.specular_color[2],
                    p.specular_strength,
                    p.emissive_color[0],
                    p.emissive_color[1],
                    p.emissive_color[2],
                    p.emissive_multiple,
                    p.shader_flags_1,
                );
            }
        }
    }
}
