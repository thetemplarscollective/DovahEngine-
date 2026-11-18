//! Recursively scan a directory tree, decode every `.dds` file's
//! level 0 in memory, and report format distribution + failures.
//!
//! Usage: `cargo run --example dds_scan -- <dir>`
//!
//! Nothing is written to disk. Exits non-zero if any file fails to
//! parse or decode.

use dovah_dds::DdsImage;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

fn scan(
    dir: &Path,
    formats: &mut HashMap<String, usize>,
    failed: &mut Vec<(String, String)>,
    ok: &mut usize,
) {
    let entries = match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect::<Vec<_>>(),
        Err(_) => return,
    };
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            scan(&path, formats, failed, ok);
            continue;
        }
        if path.extension().is_none_or(|e| e != "dds") {
            continue;
        }
        let name = path.to_string_lossy().into_owned();
        match DdsImage::open(&path).and_then(|img| {
            let decoded = img.decode(0)?;
            Ok((img, decoded))
        }) {
            Ok((img, _)) => {
                *formats.entry(format!("{:?}", img.format)).or_insert(0) += 1;
                *ok += 1;
            }
            Err(e) => failed.push((name, e.to_string())),
        }
    }
}

fn main() -> ExitCode {
    let dir = match env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: dds_scan <dir>");
            return ExitCode::from(2);
        }
    };
    let mut formats: HashMap<String, usize> = HashMap::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    let mut ok = 0usize;
    scan(Path::new(&dir), &mut formats, &mut failed, &mut ok);
    println!("decoded ok: {ok}");
    let mut counts: Vec<(String, usize)> = formats.into_iter().collect();
    counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    println!("formats:");
    for (format, count) in counts {
        println!("  {format}: {count}");
    }
    if failed.is_empty() {
        println!("all files decoded cleanly");
        ExitCode::SUCCESS
    } else {
        println!("failures: {}", failed.len());
        for (n, e) in failed.iter().take(20) {
            println!("  {n}: {e}");
        }
        ExitCode::from(1)
    }
}
