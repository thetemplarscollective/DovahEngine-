//! Skin-gap check (M12): for every skinned shape (a `BSTriShape` /
//! `BSDynamicTriShape` with a skin instance and no inline
//! vertices), verify it assembles into a mesh, and classify the
//! ones that don't.
//!
//! Usage: `cargo run -p dovah-nif --example skin_gap_check --release -- <bsa>...`

use dovah_bsa::BsaArchive;
use dovah_nif::{Block, NifFile};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let paths: Vec<String> = env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: skin_gap_check <path-to-.bsa>...");
        return ExitCode::from(2);
    }
    let mut skinned_shapes = 0usize;
    let mut assembled = 0usize;
    let mut gaps: HashMap<String, (usize, Vec<String>)> = HashMap::new();

    for path in &paths {
        let mut archive = match BsaArchive::open(path) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("failed to open {path}: {e}");
                return ExitCode::from(1);
            }
        };
        let list: Vec<String> = archive.list().iter().map(|s| s.to_string()).collect();
        let nifs: Vec<String> = list
            .iter()
            .filter(|p| p.to_lowercase().ends_with(".nif"))
            .cloned()
            .collect();
        for nif_path in &nifs {
            let Ok(bytes) = archive.extract(nif_path) else {
                continue;
            };
            let Ok(nif) = NifFile::from_bytes(&bytes) else {
                continue;
            };
            let meshes = nif.meshes();
            let skinned_names: HashMap<&str, usize> = {
                let mut m: HashMap<&str, usize> = HashMap::new();
                for mesh in &meshes {
                    if mesh.skinned {
                        *m.entry(mesh.name.as_str()).or_insert(0) += 1;
                    }
                }
                m
            };
            let mut seen: HashMap<String, usize> = HashMap::new();
            for block in &nif.blocks {
                let (shape, dynamic) = match block {
                    Block::BsTriShape(s) => (s.as_ref(), false),
                    Block::BsDynamicTriShape(d) => (&d.shape, true),
                    _ => continue,
                };
                if shape.skin < 0 || !shape.vertices.is_empty() {
                    continue;
                }
                skinned_shapes += 1;
                let name = shape.av.net.name.clone();
                let count = seen.entry(name.clone()).or_insert(0);
                *count += 1;
                if skinned_names.get(name.as_str()).copied().unwrap_or(0) >= *count {
                    assembled += 1;
                    continue;
                }
                // Classify the gap by re-checking the skin links.
                let reason = match nif.block(shape.skin) {
                    None => "skin ref out of range".to_string(),
                    Some(Block::SkinInstance(inst)) => {
                        match nif.block(inst.partition) {
                            None => "partition ref out of range".to_string(),
                            Some(Block::SkinPartition(part)) => {
                                match nif.block(inst.data) {
                                    None => "skin data ref out of range".to_string(),
                                    Some(Block::SkinData(_)) => {
                                        let tris: usize =
                                            part.partitions.iter().map(|p| p.triangles.len()).sum();
                                        if part.vertices.is_empty() {
                                            "partition has no vertices".to_string()
                                        } else if tris == 0 {
                                            "partition has no triangles".to_string()
                                        } else if dynamic {
                                            // Dynamic count mismatch?
                                            let dcount = match block {
                                                Block::BsDynamicTriShape(d) => {
                                                    d.dynamic_positions.len()
                                                }
                                                _ => 0,
                                            };
                                            if dcount != part.vertices.len() {
                                                format!(
                                                    "dynamic count {dcount} != partition {}",
                                                    part.vertices.len()
                                                )
                                            } else {
                                                "unknown (links ok)".to_string()
                                            }
                                        } else {
                                            "unknown (links ok)".to_string()
                                        }
                                    }
                                    Some(other) => {
                                        format!("skin data ref is {}", other.type_name())
                                    }
                                }
                            }
                            Some(other) => {
                                format!("partition ref is {}", other.type_name())
                            }
                        }
                    }
                    Some(other) => format!("skin ref is {}", other.type_name()),
                };
                let entry = gaps.entry(reason).or_insert((0, Vec::new()));
                entry.0 += 1;
                if entry.1.len() < 5 {
                    entry.1.push(format!("{nif_path} shape '{name}'"));
                }
            }
        }
    }

    println!("skinned shapes (no inline vertices, skin >= 0): {skinned_shapes}");
    println!("assembled into meshes: {assembled}");
    println!("gaps: {}", skinned_shapes - assembled);
    let mut kinds: Vec<(&String, &(usize, Vec<String>))> = gaps.iter().collect();
    kinds.sort_by_key(|k| Reverse(k.1 .0));
    for (reason, (count, examples)) in kinds {
        println!("  {count:>4}  {reason}");
        for example in examples {
            println!("         e.g. {example}");
        }
    }
    ExitCode::SUCCESS
}
