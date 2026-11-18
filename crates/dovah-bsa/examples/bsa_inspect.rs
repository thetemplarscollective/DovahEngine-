//! Inspect a real BSA archive: list its files and verify every file
//! extracts cleanly.
//!
//! Usage: `cargo run --example bsa_inspect -- <path-to-.bsa>`
//!
//! Prints the archive version, file count, the first few paths, then
//! extracts every file in memory (nothing is written to disk) and
//! reports how many extracted OK and the total extracted bytes. Any
//! failure is printed with its path and the program exits non-zero.

use dovah_bsa::BsaArchive;
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let path = match env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: bsa_inspect <path-to-.bsa>");
            return ExitCode::from(2);
        }
    };
    let mut archive = match BsaArchive::open(&path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("failed to open {path}: {e}");
            return ExitCode::from(1);
        }
    };
    println!("archive: {path}");
    println!("version: {}", archive.version());
    println!("files: {}", archive.len());
    // Own the paths: `extract` needs `&mut archive`.
    let list: Vec<String> = archive.list().iter().map(|s| s.to_string()).collect();
    println!("first 10 files:");
    for p in list.iter().take(10) {
        println!("  {p}");
    }

    let mut ok = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();
    let mut total_bytes: u64 = 0;
    for p in &list {
        match archive.extract(p) {
            Ok(data) => {
                ok += 1;
                total_bytes += data.len() as u64;
            }
            Err(e) => failed.push((p.to_string(), e.to_string())),
        }
    }
    println!("extracted ok: {ok}/{}", list.len());
    println!("total extracted bytes: {total_bytes}");
    if failed.is_empty() {
        println!("all files extracted cleanly");
        ExitCode::SUCCESS
    } else {
        println!("failures: {}", failed.len());
        for (p, e) in failed.iter().take(20) {
            println!("  {p}: {e}");
        }
        ExitCode::from(1)
    }
}
