//! NIF parse sweep: parse every `.nif` in one or more BSA archives
//! in memory and report parse coverage.
//!
//! Usage: `cargo run -p dovah-nif --example nif_sweep --release -- <bsa>...`
//!
//! Nothing is written to disk: each file is extracted to memory,
//! parsed with [`dovah_nif::NifFile::from_bytes`], and its meshes
//! assembled with [`dovah_nif::NifFile::meshes`] (so assembler bugs
//! surface too). Failures are grouped by error kind with example
//! paths, and the summary reports BS-version distribution, mesh
//! counts, zero-mesh files, and the unknown block types seen —
//! everything the real-data check needs to judge the loader.
//!
//! Each file is parsed inside `catch_unwind`: one bad file must not
//! kill a 25,000-file sweep.

use dovah_bsa::BsaArchive;
use dovah_nif::{Block, NifFile};
use std::collections::HashMap;
use std::env;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::ExitCode;
use std::time::Instant;

/// Failure counts by kind, with up to 5 example paths each.
#[derive(Default)]
struct Failures {
    counts: HashMap<String, usize>,
    examples: HashMap<String, Vec<String>>,
}

impl Failures {
    fn add(&mut self, kind: String, path: &str) {
        *self.counts.entry(kind.clone()).or_insert(0) += 1;
        let examples = self.examples.entry(kind).or_default();
        if examples.len() < 5 {
            examples.push(path.to_string());
        }
    }

    fn total(&self) -> usize {
        self.counts.values().sum()
    }

    fn print(&self, title: &str) {
        if self.counts.is_empty() {
            println!("{title}: none");
            return;
        }
        println!("{title}: {} total", self.total());
        let mut kinds: Vec<(&String, &usize)> = self.counts.iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(a.1));
        for (kind, count) in kinds {
            println!("  {count:>6}  {kind}");
            for example in &self.examples[kind] {
                println!("           e.g. {example}");
            }
        }
    }
}

#[derive(Default)]
struct Stats {
    archives: usize,
    files_listed: usize,
    nif_files: usize,
    extract_failed: Failures,
    parse_failed: Failures,
    panics: Failures,
    parsed_ok: usize,
    /// BS stream version -> file count.
    bs_versions: HashMap<u32, usize>,
    /// Files that parsed but assembled zero meshes.
    zero_mesh_files: usize,
    zero_mesh_examples: Vec<String>,
    /// Zero-mesh files containing a BSDynamicTriShape block (should
    /// be ~0 now the loader parses that shape type).
    zero_mesh_with_dynamic: usize,
    /// Block-type counts across zero-mesh files (type name -> files).
    zero_mesh_block_types: HashMap<String, usize>,
    /// Files containing a BSDynamicTriShape block at all.
    files_with_dynamic: usize,
    /// BSDynamicTriShape blocks parsed.
    dynamic_blocks: usize,
    /// BSDynamicTriShape blocks with trailing bytes after the
    /// dynamic section (non-zero means a layout variant).
    dynamic_trailing_nonzero: usize,
    dynamic_trailing_max: usize,
    dynamic_trailing_examples: Vec<String>,
    /// Dynamic sections whose declared size differs from
    /// stride × num_vertices (stride from desc bits 4-7).
    dynamic_size_mismatch: usize,
    dynamic_size_mismatch_examples: Vec<String>,
    /// Dynamic positions parsed, and how many fall outside their
    /// shape's bounding sphere (validates the position layout).
    dynamic_positions: usize,
    dynamic_positions_outside_sphere: usize,
    /// BSTriShape blocks with no inline vertices (skinned SSE
    /// meshes: geometry lives in the NiSkinPartition).
    skinned_bs_shapes: usize,
    /// BSDynamicTriShape blocks with no inline vertices (all of
    /// them, until the skin-partition reconstruction lands).
    dynamic_no_inline: usize,
    /// BSTriShape blocks with BOTH inline vertices and a skin
    /// instance (unknown whether this occurs; inline wins).
    skinned_shapes_with_inline: usize,
    /// NiSkinPartition blocks parsed (M12).
    skin_partitions: usize,
    /// NiSkinPartition blocks with trailing bytes after parsing
    /// (non-zero means a layout variant).
    skin_partition_trailing_nonzero: usize,
    /// NiSkinData blocks with trailing bytes after parsing.
    skin_data_trailing_nonzero: usize,
    /// Meshes reconstructed from a skin partition in bind pose.
    skinned_meshes: usize,
    total_meshes: usize,
    total_vertices: usize,
    total_triangles: usize,
    /// Unknown block type name -> block count.
    unknown_blocks: HashMap<String, usize>,
    /// Files containing at least one unknown block type.
    files_with_unknown: usize,
}

fn main() -> ExitCode {
    // Silence the default panic hook: panics are caught per file and
    // reported as a failure kind; the default hook would spam stderr
    // with a full backtrace per bad file.
    std::panic::set_hook(Box::new(|_| {}));
    let paths: Vec<String> = env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: nif_sweep <path-to-.bsa>...");
        return ExitCode::from(2);
    }
    let started = Instant::now();
    let mut stats = Stats::default();

    for path in &paths {
        let mut archive = match BsaArchive::open(path) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("failed to open {path}: {e}");
                return ExitCode::from(1);
            }
        };
        stats.archives += 1;
        // Own the paths: `extract` needs `&mut archive`.
        let list: Vec<String> = archive.list().iter().map(|s| s.to_string()).collect();
        stats.files_listed += list.len();
        let nifs: Vec<String> = list
            .iter()
            .filter(|p| p.to_lowercase().ends_with(".nif"))
            .cloned()
            .collect();
        println!(
            "{path}: version {}, {} files, {} .nif",
            archive.version(),
            list.len(),
            nifs.len()
        );
        stats.nif_files += nifs.len();

        for (i, nif_path) in nifs.iter().enumerate() {
            if i > 0 && i % 2000 == 0 {
                eprintln!("  ... {i}/{} in {}", nifs.len(), path);
            }
            let bytes = match archive.extract(nif_path) {
                Ok(b) => b,
                Err(e) => {
                    stats.extract_failed.add(e.to_string(), nif_path);
                    continue;
                }
            };
            let parsed = catch_unwind(AssertUnwindSafe(|| NifFile::from_bytes(&bytes)));
            let nif = match parsed {
                Ok(Ok(nif)) => nif,
                Ok(Err(e)) => {
                    stats.parse_failed.add(e.to_string(), nif_path);
                    continue;
                }
                Err(payload) => {
                    let msg = payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".to_string());
                    stats.panics.add(format!("panic: {msg}"), nif_path);
                    continue;
                }
            };
            stats.parsed_ok += 1;
            *stats.bs_versions.entry(nif.bs_version).or_insert(0) += 1;
            let mut saw_unknown = false;
            let mut has_dynamic = false;
            for block in &nif.blocks {
                match block {
                    Block::Unknown { type_name } => {
                        *stats.unknown_blocks.entry(type_name.clone()).or_insert(0) += 1;
                        saw_unknown = true;
                    }
                    Block::BsTriShape(shape) => {
                        if shape.vertices.is_empty() {
                            stats.skinned_bs_shapes += 1;
                        } else if shape.skin >= 0 {
                            stats.skinned_shapes_with_inline += 1;
                        }
                    }
                    Block::SkinPartition(partition) => {
                        stats.skin_partitions += 1;
                        if partition.trailing_bytes > 0 {
                            stats.skin_partition_trailing_nonzero += 1;
                        }
                    }
                    Block::SkinData(data) => {
                        if data.trailing_bytes > 0 {
                            stats.skin_data_trailing_nonzero += 1;
                        }
                    }
                    Block::BsDynamicTriShape(dynamic) => {
                        has_dynamic = true;
                        stats.dynamic_blocks += 1;
                        if dynamic.shape.vertices.is_empty() {
                            stats.dynamic_no_inline += 1;
                        }
                        if dynamic.trailing_bytes > 0 {
                            stats.dynamic_trailing_nonzero += 1;
                            stats.dynamic_trailing_max =
                                stats.dynamic_trailing_max.max(dynamic.trailing_bytes);
                            if stats.dynamic_trailing_examples.len() < 5 {
                                stats.dynamic_trailing_examples.push(format!(
                                    "{nif_path} ({} trailing bytes)",
                                    dynamic.trailing_bytes
                                ));
                            }
                        }
                        let stride = (((dynamic.shape.vertex_desc >> 4) & 0xF) * 4) as usize;
                        let expected = stride * dynamic.shape.num_vertices as usize;
                        if dynamic.dynamic_data_size as usize != expected {
                            stats.dynamic_size_mismatch += 1;
                            if stats.dynamic_size_mismatch_examples.len() < 5 {
                                stats.dynamic_size_mismatch_examples.push(format!(
                                    "{nif_path} (declared {}, stride {stride} x {} verts = {expected})",
                                    dynamic.dynamic_data_size,
                                    dynamic.shape.num_vertices
                                ));
                            }
                        }
                        let (center, radius) = dynamic.shape.bounding_sphere;
                        for pos in &dynamic.dynamic_positions {
                            stats.dynamic_positions += 1;
                            let dx = pos[0] - center[0];
                            let dy = pos[1] - center[1];
                            let dz = pos[2] - center[2];
                            // 1% tolerance: bounding spheres are
                            // approximate, and float rounding adds
                            // noise at the boundary.
                            if dx * dx + dy * dy + dz * dz > radius * radius * 1.0201 {
                                stats.dynamic_positions_outside_sphere += 1;
                            }
                        }
                    }
                    _ => {}
                }
            }
            if saw_unknown {
                stats.files_with_unknown += 1;
            }
            if has_dynamic {
                stats.files_with_dynamic += 1;
            }
            // Assemble meshes: this exercises the shape/data/shader
            // resolution the renderer depends on.
            let assembled = catch_unwind(AssertUnwindSafe(|| nif.meshes()));
            match assembled {
                Ok(meshes) => {
                    if meshes.is_empty() {
                        stats.zero_mesh_files += 1;
                        if has_dynamic {
                            stats.zero_mesh_with_dynamic += 1;
                        }
                        // Block-type composition of zero-mesh files:
                        // what ARE these files, if not geometry?
                        let mut seen: Vec<String> = nif
                            .blocks
                            .iter()
                            .map(|b| b.type_name().to_string())
                            .collect();
                        seen.sort();
                        seen.dedup();
                        for name in seen {
                            *stats.zero_mesh_block_types.entry(name).or_insert(0) += 1;
                        }
                        if stats.zero_mesh_examples.len() < 5 {
                            stats.zero_mesh_examples.push(nif_path.clone());
                        }
                    }
                    for mesh in &meshes {
                        stats.total_meshes += 1;
                        if mesh.skinned {
                            stats.skinned_meshes += 1;
                        }
                        stats.total_vertices += mesh.vertices.len();
                        stats.total_triangles += mesh.indices.len() / 3;
                    }
                }
                Err(payload) => {
                    let msg = payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".to_string());
                    stats
                        .panics
                        .add(format!("panic in meshes(): {msg}"), nif_path);
                }
            }
        }
    }

    println!();
    println!("=== NIF sweep summary ===");
    println!("archives: {}", stats.archives);
    println!("files listed: {}", stats.files_listed);
    println!(".nif files: {}", stats.nif_files);
    println!("parsed ok: {}", stats.parsed_ok);
    stats.extract_failed.print("extract failures");
    stats.parse_failed.print("parse failures");
    stats.panics.print("panics");
    println!("BS stream versions:");
    let mut versions: Vec<(&u32, &usize)> = stats.bs_versions.iter().collect();
    versions.sort();
    for (version, count) in versions {
        println!("  BS {version}: {count} files");
    }
    println!(
        "files with zero meshes: {} (of {} parsed)",
        stats.zero_mesh_files, stats.parsed_ok
    );
    println!(
        "  of those, with a BSDynamicTriShape block: {}",
        stats.zero_mesh_with_dynamic
    );
    println!(
        "files with a BSDynamicTriShape block (any): {}",
        stats.files_with_dynamic
    );
    println!(
        "BSDynamicTriShape blocks: {} parsed, {} with trailing bytes (max {} bytes)",
        stats.dynamic_blocks, stats.dynamic_trailing_nonzero, stats.dynamic_trailing_max
    );
    for example in &stats.dynamic_trailing_examples {
        println!("  e.g. {example}");
    }
    println!(
        "dynamic sections with size != stride x num_vertices: {}",
        stats.dynamic_size_mismatch
    );
    for example in &stats.dynamic_size_mismatch_examples {
        println!("  e.g. {example}");
    }
    println!(
        "dynamic positions parsed: {}, outside bounding sphere: {}",
        stats.dynamic_positions, stats.dynamic_positions_outside_sphere
    );
    println!(
        "shapes with no inline vertices (geometry in skin partition): {} BSTriShape, {} BSDynamicTriShape",
        stats.skinned_bs_shapes, stats.dynamic_no_inline
    );
    println!(
        "BSTriShape blocks with BOTH inline vertices and a skin instance: {}",
        stats.skinned_shapes_with_inline
    );
    println!(
        "NiSkinPartition blocks: {}, with trailing bytes: {}; NiSkinData blocks with trailing bytes: {}",
        stats.skin_partitions,
        stats.skin_partition_trailing_nonzero,
        stats.skin_data_trailing_nonzero
    );
    println!(
        "meshes reconstructed from skin partitions (bind pose): {}",
        stats.skinned_meshes
    );
    let mut zero_types: Vec<(&String, &usize)> = stats.zero_mesh_block_types.iter().collect();
    zero_types.sort_by(|a, b| b.1.cmp(a.1));
    println!("top block types in zero-mesh files:");
    for (name, count) in zero_types.iter().take(12) {
        println!("  {count:>6}  {name}");
    }
    for example in &stats.zero_mesh_examples {
        println!("  e.g. {example}");
    }
    println!(
        "meshes assembled: {} ({} vertices, {} triangles)",
        stats.total_meshes, stats.total_vertices, stats.total_triangles
    );
    println!(
        "files with unknown block types: {}",
        stats.files_with_unknown
    );
    let mut unknown: Vec<(&String, &usize)> = stats.unknown_blocks.iter().collect();
    unknown.sort_by(|a, b| b.1.cmp(a.1));
    println!("top unknown block types ({} distinct):", unknown.len());
    for (name, count) in unknown.iter().take(20) {
        println!("  {count:>6}  {name}");
    }
    println!("elapsed: {:.1}s", started.elapsed().as_secs_f32());

    if stats.extract_failed.total() + stats.parse_failed.total() + stats.panics.total() == 0 {
        println!("ALL NIF FILES PARSED CLEANLY");
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
