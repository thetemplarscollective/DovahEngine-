//! List every file in a BSA whose path contains a substring
//! (case-insensitive). bsa_inspect only prints the first 10
//! files, which is useless for finding a file — this prints
//! all matches.
//!
//! Usage: cargo run -p dovah-bsa --example bsa_grep --
//!   <path-to-.bsa> <substring> [substring...]

use dovah_bsa::BsaArchive;
use std::env;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: bsa_grep <bsa-path> <substring> [substring...]");
        return;
    }
    let needles: Vec<String> = args[1..].iter().map(|s| s.to_lowercase()).collect();
    let archive = BsaArchive::open(&args[0]).expect("open BSA");
    let mut hits = 0;
    for p in archive.list() {
        let lower = p.to_lowercase();
        if needles.iter().all(|n| lower.contains(n)) {
            println!("{p}");
            hits += 1;
        }
    }
    eprintln!("{hits} matches in {} files", archive.len());
}
