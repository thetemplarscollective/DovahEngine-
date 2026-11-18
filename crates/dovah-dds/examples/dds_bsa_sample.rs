//! Real-data check: decode a sample of DDS files straight out of a
//! BSA archive.
//!
//! Usage: `cargo run --example dds_bsa_sample -- <path-to-.bsa> [count]`
//!
//! Takes a spread sample of `.dds` files across the archive (not
//! just the first N), extracts each one in memory, parses it with
//! dovah-dds, and decodes every mip level. Prints one line per file
//! (format, dimensions, mip count) and a summary. Optionally writes
//! the sampled DDS files to a directory for cross-checking against
//! another decoder:
//!
//! `cargo run --example dds_bsa_sample -- <bsa> [count] <out-dir>`

use dovah_bsa::BsaArchive;
use dovah_dds::DdsImage;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 || args.len() > 4 {
        eprintln!("usage: dds_bsa_sample <path-to-.bsa> [count] [out-dir]");
        return ExitCode::from(2);
    }
    let count: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(24);
    let out_dir = args.get(3).map(Path::new);
    if let Some(dir) = out_dir {
        if let Err(e) = fs::create_dir_all(dir) {
            eprintln!("cannot create {}: {e}", dir.display());
            return ExitCode::from(1);
        }
    }
    let mut archive = match BsaArchive::open(&args[1]) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("failed to open {}: {e}", args[1]);
            return ExitCode::from(1);
        }
    };
    let dds_files: Vec<String> = archive
        .list()
        .iter()
        .filter(|p| p.to_lowercase().ends_with(".dds"))
        .map(|s| s.to_string())
        .collect();
    println!("dds files in archive: {}", dds_files.len());
    if dds_files.is_empty() {
        return ExitCode::SUCCESS;
    }
    // Spread sample across the whole list.
    let step = (dds_files.len() as f64 / count as f64).max(1.0);
    let mut sample: Vec<&String> = Vec::new();
    let mut i = 0.0;
    while (i as usize) < dds_files.len() && sample.len() < count {
        sample.push(&dds_files[i as usize]);
        i += step;
    }
    let mut ok = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();
    let mut formats: HashMap<String, usize> = HashMap::new();
    let mut total_mips = 0usize;
    for path in sample {
        match archive.extract(path) {
            Ok(bytes) => {
                if let Some(dir) = out_dir {
                    let flat = path.replace('/', "_");
                    let _ = fs::write(dir.join(flat), &bytes);
                }
                match DdsImage::from_bytes(&bytes) {
                    Ok(img) => {
                        let mut decoded_levels = 0usize;
                        let mut decode_err: Option<String> = None;
                        for level in 0..img.mip_map_count {
                            match img.decode(level) {
                                Ok(_) => decoded_levels += 1,
                                Err(e) => {
                                    decode_err = Some(e.to_string());
                                    break;
                                }
                            }
                        }
                        if decoded_levels == img.mip_map_count as usize {
                            *formats.entry(format!("{:?}", img.format)).or_insert(0) += 1;
                            total_mips += decoded_levels;
                            println!(
                                "{path} | {:?} | {}x{} | mips={} | ok",
                                img.format, img.width, img.height, img.mip_map_count
                            );
                            ok += 1;
                        } else {
                            failed.push((
                                path.to_string(),
                                format!(
                                    "decoded {decoded_levels}/{} levels: {}",
                                    img.mip_map_count,
                                    decode_err.unwrap_or_default()
                                ),
                            ));
                        }
                    }
                    Err(e) => failed.push((path.to_string(), e.to_string())),
                }
            }
            Err(e) => failed.push((path.to_string(), e.to_string())),
        }
    }
    println!("decoded ok: {ok}");
    println!("formats: {formats:?}");
    println!("total mip levels decoded: {total_mips}");
    if failed.is_empty() {
        println!("all sampled files decoded cleanly");
        ExitCode::SUCCESS
    } else {
        println!("failures: {}", failed.len());
        for (p, e) in failed.iter().take(20) {
            println!("  {p}: {e}");
        }
        ExitCode::from(1)
    }
}
