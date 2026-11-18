//! Extract one file from a BSA archive.
//!
//! Usage: `cargo run --example bsa_extract_one -- <path-to-.bsa> <path-in-archive> <out-file>`

use dovah_bsa::BsaArchive;
use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: bsa_extract_one <path-to-.bsa> <path-in-archive> <out-file>");
        return ExitCode::from(2);
    }
    let mut archive = match BsaArchive::open(&args[1]) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("failed to open {}: {e}", args[1]);
            return ExitCode::from(1);
        }
    };
    match archive.extract(&args[2]) {
        Ok(data) => match fs::write(&args[3], &data) {
            Ok(()) => {
                println!(
                    "extracted {} ({} bytes) to {}",
                    args[2],
                    data.len(),
                    args[3]
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("write failed: {e}");
                ExitCode::from(1)
            }
        },
        Err(e) => {
            eprintln!("extract failed: {e}");
            ExitCode::from(1)
        }
    }
}
