//! Sweep every NIF in Meshes0/1 and count `NiAlphaProperty` flag
//! combinations: how many shapes are test-only, blend-only,
//! test+blend, and additive. Answers the transparent pass's open
//! question — does the cutoff-wins rule (test+blend renders as
//! cutout) bite anywhere, and how much of the game goes through
//! the blend path at all?
//!
//! Usage: `cargo run -p dovah-app --example alpha_flag_sweep`

use dovah_bsa::BsaArchive;
use dovah_nif::{alpha_blend_mode, alpha_test_cutoff, AlphaBlend, Block, NifFile};
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("test-data"));
    let mut archives: Vec<BsaArchive<File>> = Vec::new();
    for name in ["Skyrim_-_Meshes0.bsa", "Skyrim_-_Meshes1.bsa"] {
        let path = root.join(name);
        if path.exists() {
            archives.push(BsaArchive::open(&path).unwrap_or_else(|e| panic!("open {name}: {e}")));
        }
    }
    assert!(!archives.is_empty(), "no Meshes BSA found under {root:?}");

    let mut files = 0usize;
    let mut shapes = 0usize;
    let mut no_alpha = 0usize;
    let mut test_only = 0usize;
    let mut blend_only = 0usize;
    let mut test_and_blend = 0usize;
    let mut additive = 0usize;
    let mut flag_counts: HashMap<u16, usize> = HashMap::new();

    for archive in &mut archives {
        let names: Vec<String> = archive
            .list()
            .into_iter()
            .filter(|n| n.to_lowercase().ends_with(".nif"))
            .map(|n| n.to_string())
            .collect();
        for name in &names {
            let Ok(bytes) = archive.extract(name) else {
                continue;
            };
            let Ok(nif) = NifFile::from_bytes(&bytes) else {
                continue;
            };
            files += 1;
            // Alpha flags per shape: walk the meshes (they carry
            // the resolved cutoff + blend) — but the raw flags
            // need the alpha property blocks, so count from the
            // blocks and join via the meshes' own decode.
            for mesh in nif.meshes() {
                shapes += 1;
                let blend = mesh.alpha_blend;
                let cutoff = mesh.alpha_cutoff.is_some();
                match (cutoff, blend) {
                    (true, AlphaBlend::Opaque) => test_only += 1,
                    (false, AlphaBlend::Blend) => blend_only += 1,
                    (true, _) => test_and_blend += 1,
                    (false, AlphaBlend::Additive) => additive += 1,
                    (false, AlphaBlend::Opaque) => no_alpha += 1,
                }
            }
            for block in &nif.blocks {
                if let Block::AlphaProperty(prop) = block {
                    *flag_counts.entry(prop.flags).or_insert(0) += 1;
                }
            }
        }
    }

    println!("files: {files}");
    println!("shapes: {shapes}");
    println!("  no alpha property / opaque: {no_alpha}");
    println!("  test-only (A2C cutout):     {test_only}");
    println!("  blend-only (transparent):   {blend_only}");
    println!("  additive (transparent):     {additive}");
    println!("  test+blend (cutoff wins):   {test_and_blend}");
    println!("distinct alpha flag values: {}", flag_counts.len());
    let mut flags: Vec<(u16, usize)> = flag_counts.into_iter().collect();
    flags.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    for (flags, n) in flags.iter().take(12) {
        let cutoff = alpha_test_cutoff(*flags, 128).is_some();
        let blend = alpha_blend_mode(*flags);
        println!("  flags 0x{flags:04x}: {n:6} test={cutoff} blend={blend:?}");
    }
}
