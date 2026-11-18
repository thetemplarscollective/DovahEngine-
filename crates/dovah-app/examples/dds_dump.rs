//! Dump DDS textures from the BSAs to PNG files, so a texture can
//! be LOOKED at instead of summarized by its mean color.
//! Usage: cargo run -p dovah-app --example dds_dump --
//!        <dds-path-in-bsa> [dds-path...]
//! Output: screenshots/dds-<basename>.png

use dovah_bsa::BsaArchive;
use std::collections::HashMap;
use std::fs::{self, File};

fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(!args.is_empty(), "pass DDS paths");
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
    for path in &args {
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
        let base = path
            .replace('\\', "/")
            .rsplit('/')
            .next()
            .unwrap_or("out")
            .replace(".dds", "")
            .replace(".DDS", "");
        let out = format!("screenshots/dds-{base}.png");
        let rgba = image::RgbaImage::from_raw(img.width, img.height, img.data)
            .expect("rgba buffer size");
        rgba.save(&out).expect("save png");
        println!("{path}: {}x{} -> {out}", img.width, img.height);
    }
}
