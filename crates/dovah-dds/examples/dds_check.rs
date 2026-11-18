//! Decode every `.dds` file in a directory to raw RGBA and write the
//! results out for cross-checking against another decoder.
//!
//! Usage: `cargo run --example dds_check -- <in-dir> <out-dir>`
//!
//! For each file prints `name | format | WxH | mips | status` and
//! writes `<name>.rgba` (row-major RGBA8, level 0) to the output
//! directory. Exits non-zero if any file fails to parse or decode.

use dovah_dds::DdsImage;
use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: dds_check <in-dir> <out-dir>");
        return ExitCode::from(2);
    }
    let in_dir = Path::new(&args[1]);
    let out_dir = Path::new(&args[2]);
    if fs::create_dir_all(out_dir).is_err() {
        eprintln!("cannot create output dir {}", out_dir.display());
        return ExitCode::from(1);
    }
    let mut entries: Vec<_> = match fs::read_dir(in_dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(e) => {
            eprintln!("cannot read {}: {e}", in_dir.display());
            return ExitCode::from(1);
        }
    };
    entries.sort_by_key(|e| e.file_name());
    let mut ok = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "dds") {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let stem = name.trim_end_matches(".dds").to_string();
        match DdsImage::open(&path).and_then(|img| {
            let decoded = img.decode(0)?;
            Ok((img, decoded))
        }) {
            Ok((img, decoded)) => {
                let out_path = out_dir.join(format!("{stem}.rgba"));
                match fs::write(&out_path, &decoded.data) {
                    Ok(()) => {
                        println!(
                            "{name} | {:?} | {}x{} | mips={} | ok",
                            img.format, img.width, img.height, img.mip_map_count
                        );
                        ok += 1;
                    }
                    Err(e) => failed.push((name, e.to_string())),
                }
            }
            Err(e) => failed.push((name, e.to_string())),
        }
    }
    println!("decoded ok: {ok}");
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
