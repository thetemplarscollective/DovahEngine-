//! Full sweep: extract EVERY `.dds` in one or more BSA archives in
//! memory, parse it, decode mip level 0, and report the format
//! distribution + failures. Nothing is written to disk except
//! (optionally) BC7 samples for cross-checking.
//!
//! BC7 files get every mip level decoded, not just level 0 —
//! BC7 is the format no real-data sweep had ever found before M10,
//! so any BC7 file found here is decoded end to end. With
//! `--bc7-out DIR`, the first few BC7 files are written out flat
//! for an independent decode check (e.g. ffmpeg's DDS decoder).
//!
//! Usage:
//! `cargo run -p dovah-dds --example dds_bsa_sweep -- <bsa> [bsa...] [--bc7-out DIR]`
//!
//! Exits non-zero if any file fails to extract, parse, or decode.

use dovah_bsa::BsaArchive;
use dovah_dds::{DdsImage, Format};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

const BC7_SAMPLE_LIMIT: usize = 5;

struct Sweep {
    dds_total: usize,
    ok: usize,
    failed: Vec<(String, String)>,
    formats: HashMap<String, usize>,
    bc7_files: Vec<String>,
    bc7_levels_decoded: usize,
}

impl Sweep {
    fn new() -> Self {
        Sweep {
            dds_total: 0,
            ok: 0,
            failed: Vec::new(),
            formats: HashMap::new(),
            bc7_files: Vec::new(),
            bc7_levels_decoded: 0,
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut bsas: Vec<PathBuf> = Vec::new();
    let mut bc7_out: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--bc7-out" {
            match it.next() {
                Some(d) => bc7_out = Some(PathBuf::from(d)),
                None => {
                    eprintln!("--bc7-out needs a directory");
                    return ExitCode::from(2);
                }
            }
        } else {
            bsas.push(PathBuf::from(a));
        }
    }
    if bsas.is_empty() {
        eprintln!("usage: dds_bsa_sweep <bsa> [bsa...] [--bc7-out DIR]");
        return ExitCode::from(2);
    }
    if let Some(dir) = &bc7_out {
        if let Err(e) = fs::create_dir_all(dir) {
            eprintln!("cannot create {}: {e}", dir.display());
            return ExitCode::from(1);
        }
    }

    let mut total = Sweep::new();
    let mut all_ok = true;
    for bsa_path in &bsas {
        let name = bsa_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        println!("\n=== {name} ===");
        let mut archive = match BsaArchive::open(bsa_path) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("failed to open {}: {e}", bsa_path.display());
                all_ok = false;
                continue;
            }
        };
        let files = archive.list();
        let dds_files: Vec<String> = files
            .iter()
            .filter(|p| p.to_lowercase().ends_with(".dds"))
            .map(|s| s.to_string())
            .collect();
        println!(
            "files in archive: {}, .dds: {}",
            files.len(),
            dds_files.len()
        );
        let mut sweep = Sweep::new();
        sweep.dds_total = dds_files.len();
        for path in &dds_files {
            match archive.extract(path) {
                Ok(bytes) => match DdsImage::from_bytes(&bytes) {
                    Ok(img) => {
                        let is_bc7 = matches!(img.format, Format::Bc7);
                        if is_bc7 {
                            sweep.bc7_files.push(path.clone());
                            // BC7: decode every mip level.
                            let mut levels_ok = 0usize;
                            let mut err: Option<String> = None;
                            for level in 0..img.mip_map_count {
                                match img.decode(level) {
                                    Ok(_) => levels_ok += 1,
                                    Err(e) => {
                                        err = Some(e.to_string());
                                        break;
                                    }
                                }
                            }
                            if levels_ok == img.mip_map_count as usize {
                                sweep.bc7_levels_decoded += levels_ok;
                                *sweep
                                    .formats
                                    .entry(format!("{:?}", img.format))
                                    .or_insert(0) += 1;
                                sweep.ok += 1;
                                // Write out the first few BC7 files
                                // for an independent decode check.
                                if let Some(dir) = &bc7_out {
                                    if sweep.bc7_files.len() <= BC7_SAMPLE_LIMIT {
                                        let flat = path.replace(['/', '\\'], "_");
                                        let _ = fs::write(dir.join(flat), &bytes);
                                    }
                                }
                            } else {
                                sweep.failed.push((
                                    format!("{name}:{path}"),
                                    format!(
                                        "BC7 decoded {levels_ok}/{} levels: {}",
                                        img.mip_map_count,
                                        err.unwrap_or_default()
                                    ),
                                ));
                            }
                        } else {
                            match img.decode(0) {
                                Ok(_) => {
                                    *sweep
                                        .formats
                                        .entry(format!("{:?}", img.format))
                                        .or_insert(0) += 1;
                                    sweep.ok += 1;
                                }
                                Err(e) => {
                                    sweep.failed.push((format!("{name}:{path}"), e.to_string()))
                                }
                            }
                        }
                    }
                    Err(e) => sweep
                        .failed
                        .push((format!("{name}:{path}"), format!("parse: {e}"))),
                },
                Err(e) => sweep
                    .failed
                    .push((format!("{name}:{path}"), format!("extract: {e}"))),
            }
        }
        println!("decoded ok: {} / {}", sweep.ok, sweep.dds_total);
        let mut counts: Vec<(String, usize)> = sweep.formats.clone().into_iter().collect();
        counts.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        println!("formats:");
        for (format, count) in &counts {
            println!("  {format}: {count}");
        }
        if !sweep.bc7_files.is_empty() {
            println!(
                "BC7 files: {} ({} mip levels decoded)",
                sweep.bc7_files.len(),
                sweep.bc7_levels_decoded
            );
            for f in sweep.bc7_files.iter().take(10) {
                println!("  BC7: {f}");
            }
        }
        if !sweep.failed.is_empty() {
            println!("failures: {}", sweep.failed.len());
            for (p, e) in sweep.failed.iter().take(20) {
                println!("  {p}: {e}");
            }
            all_ok = false;
        }
        total.dds_total += sweep.dds_total;
        total.ok += sweep.ok;
        total.failed.extend(sweep.failed);
        for (f, c) in sweep.formats {
            *total.formats.entry(f).or_insert(0) += c;
        }
        total.bc7_files.extend(sweep.bc7_files);
        total.bc7_levels_decoded += sweep.bc7_levels_decoded;
    }

    println!("\n=== TOTAL across {} archives ===", bsas.len());
    println!("dds decoded ok: {} / {}", total.ok, total.dds_total);
    let mut counts: Vec<(String, usize)> = total.formats.into_iter().collect();
    counts.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    println!("formats:");
    for (format, count) in counts {
        println!("  {format}: {count}");
    }
    println!(
        "BC7 total: {} files, {} mip levels decoded",
        total.bc7_files.len(),
        total.bc7_levels_decoded
    );
    if total.failed.is_empty() && all_ok {
        println!("all files decoded cleanly");
        ExitCode::SUCCESS
    } else {
        println!("total failures: {}", total.failed.len());
        ExitCode::from(1)
    }
}
