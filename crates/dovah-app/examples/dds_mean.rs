//! M11 probe: print mean RGB + dimensions of DDS files from the
//! BSAs, to compare rendered pixels against source texture pixels.
//! Usage: cargo run -p dovah-app --example dds_mean -- <dds-path>...

use dovah_bsa::BsaArchive;
use std::collections::HashMap;
use std::fs::{self, File};

fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(!args.is_empty(), "pass DDS paths or --list <substring>");
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut archives: Vec<BsaArchive<File>> = Vec::new();
    let mut paths: Vec<_> = fs::read_dir("test-data")
        .expect("read data dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .map(|e| e.eq_ignore_ascii_case("bsa"))
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    for path in paths {
        match BsaArchive::open(&path) {
            Ok(archive) => {
                let idx = archives.len();
                for stored in archive.list() {
                    index.entry(normalize(stored)).or_insert(idx);
                }
                archives.push(archive);
            }
            Err(e) => eprintln!("warning: could not open {}: {e}", path.display()),
        }
    }
    if args.len() == 2 && args[0] == "--list" {
        let needle = normalize(&args[1]);
        let mut hits: Vec<&String> = index.keys().filter(|k| k.contains(&needle)).collect();
        hits.sort();
        for h in hits {
            println!("{h}");
        }
        return;
    }
    let want = args;
    for path in &want {
        let Some(&idx) = index.get(&normalize(path)) else {
            println!("{path}: NOT IN ARCHIVES");
            continue;
        };
        let stored: String = archives[idx]
            .list()
            .into_iter()
            .find(|s| normalize(s) == normalize(path))
            .unwrap()
            .to_string();
        let bytes = match archives[idx].extract(&stored) {
            Ok(b) => b,
            Err(e) => {
                println!("{path}: extract failed: {e}");
                continue;
            }
        };
        let dds = match dovah_dds::DdsImage::from_bytes(&bytes) {
            Ok(d) => d,
            Err(e) => {
                println!("{path}: parse failed: {e}");
                continue;
            }
        };
        let img = match dds.decode(0) {
            Ok(i) => i,
            Err(e) => {
                println!("{path}: decode failed: {e}");
                continue;
            }
        };
        let n = (img.width * img.height) as usize;
        let mut sr = 0u64;
        let mut sg = 0u64;
        let mut sb = 0u64;
        for p in 0..n {
            sr += img.data[p * 4] as u64;
            sg += img.data[p * 4 + 1] as u64;
            sb += img.data[p * 4 + 2] as u64;
        }
        println!(
            "{path}: {}x{} mean RGB=({:.0},{:.0},{:.0})",
            img.width,
            img.height,
            sr as f64 / n as f64,
            sg as f64 / n as f64,
            sb as f64 / n as f64
        );
    }
}
