//! Extract every file from a BSA archive to a directory.
//!
//! Usage: `cargo run --example bsa_extract -- <path-to-.bsa> <out-dir>`
//!
//! Directory structure inside the archive is recreated under the
//! output directory. Prints a summary and exits non-zero if any file
//! fails to extract.

use dovah_bsa::BsaArchive;
use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: bsa_extract <path-to-.bsa> <out-dir>");
        return ExitCode::from(2);
    }
    let path = &args[1];
    let out_dir = Path::new(&args[2]);
    let mut archive = match BsaArchive::open(path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("failed to open {path}: {e}");
            return ExitCode::from(1);
        }
    };
    let list: Vec<String> = archive.list().iter().map(|s| s.to_string()).collect();
    let mut ok = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();
    for p in &list {
        let dest = out_dir.join(p);
        match archive.extract(p) {
            Ok(data) => {
                if let Some(parent) = dest.parent() {
                    if let Err(e) = fs::create_dir_all(parent) {
                        failed.push((p.to_string(), e.to_string()));
                        continue;
                    }
                }
                match fs::write(&dest, &data) {
                    Ok(()) => ok += 1,
                    Err(e) => failed.push((p.to_string(), e.to_string())),
                }
            }
            Err(e) => failed.push((p.to_string(), e.to_string())),
        }
    }
    println!("extracted ok: {ok}/{}", list.len());
    if failed.is_empty() {
        println!("all files extracted cleanly to {}", out_dir.display());
        ExitCode::SUCCESS
    } else {
        println!("failures: {}", failed.len());
        for (p, e) in failed.iter().take(20) {
            println!("  {p}: {e}");
        }
        ExitCode::from(1)
    }
}
