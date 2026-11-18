//! M15 probe: what open angle does a door's own animation use?
//!
//! Door NIFs carry their open/close animation as
//! NiControllerSequence + NiTransformInterpolator +
//! NiTransformData blocks (dovah-nif keeps them as Unknown, so
//! this probe parses the NIF header itself to find the
//! NiTransformData blocks and reads their rotation keys raw).
//!
//! Proven layout of NiTransformData (20.2.0.7) for these doors:
//! the rotation is XYZ Euler (type 4), stored as THREE key
//! groups (X, Y, Z), each: u32 numKeys, u32 type (2 =
//! quadratic: time, value, fwd tangent, bwd tangent per key).
//! Then u32 numTranslationKeys, u32 numScaleKeys (both 0 here).
//! The Z group's end value is the swing angle in radians.
//!
//! Usage: cargo run -p dovah-app --example door_anim -- <nif-path>
//! e.g. meshes\Architecture\Farmhouse\FarmhouseLDoor01.nif

use dovah_bsa::BsaArchive;
use std::collections::HashMap;
use std::fs::{self, File};

fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> u8 {
        let v = self.bytes[self.pos];
        self.pos += 1;
        v
    }
    fn u16(&mut self) -> u16 {
        let v = u16::from_le_bytes(self.bytes[self.pos..self.pos + 2].try_into().unwrap());
        self.pos += 2;
        v
    }
    fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes(self.bytes[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        v
    }
    fn f32(&mut self) -> f32 {
        let v = f32::from_le_bytes(self.bytes[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        v
    }
    fn line(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
            self.pos += 1;
        }
        let s = String::from_utf8_lossy(&self.bytes[start..self.pos]).to_string();
        self.pos += 1; // the newline
        s
    }
    fn sized_string(&mut self) -> String {
        let len = self.u32() as usize;
        if len > 10_000 {
            panic!(
                "sized_string: absurd len {len} at pos {} (bytes: {:02x?})",
                self.pos,
                &self.bytes[self.pos.saturating_sub(16)..(self.pos + 16).min(self.bytes.len())]
            );
        }
        let s = String::from_utf8_lossy(&self.bytes[self.pos..self.pos + len]).to_string();
        self.pos += len;
        s
    }
    fn export_string(&mut self) -> String {
        // u8 length + bytes (trailing NUL stripped).
        let len = self.u8() as usize;
        let mut s = String::from_utf8_lossy(&self.bytes[self.pos..self.pos + len]).to_string();
        self.pos += len;
        if s.ends_with('\0') {
            s.pop();
        }
        s
    }
}

fn main() {
    let nif_path = std::env::args().nth(1).expect("NIF path argument");
    // Find + extract the NIF from the Meshes BSAs.
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut archives: Vec<BsaArchive<File>> = Vec::new();
    // Only the Meshes BSAs hold NIFs — opening every BSA (the
    // 5.5 GB of Textures archives included) just to build the
    // file index is minutes of I/O for a 9 KB file.
    let mut paths: Vec<_> = fs::read_dir("test-data")
        .expect("read data dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .map(|n| {
                    let n = n.to_string_lossy().to_lowercase();
                    n.contains("meshes") && n.ends_with(".bsa")
                })
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    for path in paths {
        if let Ok(archive) = BsaArchive::open(&path) {
            let idx = archives.len();
            for stored in archive.list() {
                index.entry(normalize(stored)).or_insert(idx);
            }
            archives.push(archive);
        }
    }
    let idx = *index.get(&normalize(&nif_path)).expect("NIF in BSA index");
    let stored: String = archives[idx]
        .list()
        .into_iter()
        .find(|s| normalize(s) == normalize(&nif_path))
        .expect("stored path")
        .to_string();
    let bytes = archives[idx].extract(&stored).expect("extract door NIF");
    println!("NIF: {nif_path} ({} bytes)", bytes.len());

    let mut r = Reader {
        bytes: &bytes,
        pos: 0,
    };
    let header = r.line();
    println!("header: {header} (pos {})", r.pos);
    let version = r.u32();
    let _endian = r.u8();
    let user_version = r.u32();
    let num_blocks = r.u32() as usize;
    println!(
        "version 0x{version:08x} user {user_version} blocks {num_blocks} (pos {})",
        r.pos
    );
    let bs_version = r.u32();
    let author = r.export_string();
    let _process = r.export_string();
    let _export = r.export_string();
    println!("bs_version {bs_version} author={author:?}");
    let num_block_types = r.u16() as usize;
    let mut block_types = Vec::with_capacity(num_block_types);
    for _ in 0..num_block_types {
        block_types.push(r.sized_string());
    }
    let mut block_type_index = Vec::with_capacity(num_blocks);
    for _ in 0..num_blocks {
        block_type_index.push(r.u16() as usize);
    }
    let mut block_sizes = Vec::with_capacity(num_blocks);
    for _ in 0..num_blocks {
        block_sizes.push(r.u32() as usize);
    }
    let num_strings = r.u32() as usize;
    let _max_string_length = r.u32();
    let mut strings = Vec::with_capacity(num_strings);
    for _ in 0..num_strings {
        strings.push(r.sized_string());
    }
    println!("strings: {strings:?}");
    let num_groups = r.u32();
    for _ in 0..num_groups {
        let _ = r.u32();
    }
    // Blocks start here.
    let blocks_start = r.pos;
    let mut offsets = Vec::with_capacity(num_blocks);
    let mut off = blocks_start;
    for size in &block_sizes {
        offsets.push(off);
        off += size;
    }
    println!(
        "blocks start at byte {blocks_start}, end at {off} (file {})",
        bytes.len()
    );

    // Find NiTransformData blocks and parse their rotation keys.
    // Proven layout: u32 numKeys (1, placeholder for XYZ), u32
    // type (4 = XYZ), then THREE key groups (X, Y, Z), each:
    // u32 numKeys, u32 type (2 = quadratic), then numKeys x
    // (f32 time, f32 value, f32 fwd tangent, f32 bwd tangent).
    // Then u32 numTranslationKeys, u32 numScaleKeys.
    for (i, &ti) in block_type_index.iter().enumerate() {
        if block_types[ti] != "NiTransformData" {
            continue;
        }
        let start = offsets[i];
        let size = block_sizes[i];
        println!("\nblock [{i}] NiTransformData at byte {start}, size {size}");
        let mut br = Reader {
            bytes: &bytes[start..start + size],
            pos: 0,
        };
        let num_rot = br.u32();
        let rot_type = br.u32();
        println!("  rotation: num={num_rot} type={rot_type} (4 = XYZ Euler)");
        if rot_type != 4 {
            println!("  (not XYZ — cannot parse)");
            continue;
        }
        for axis in ['X', 'Y', 'Z'] {
            let n = br.u32();
            let t = br.u32();
            if n > 10_000 {
                println!("  {axis}: absurd key count {n} — layout misread, aborting block");
                break;
            }
            print!("  {axis}: n={n} type={t} keys:");
            let mut first = 0.0f32;
            let mut last = 0.0f32;
            let mut last_time = 0.0f32;
            for k in 0..n {
                let time = br.f32();
                let value = br.f32();
                let _fwd = br.f32();
                let _bwd = br.f32();
                if k == 0 {
                    first = value;
                }
                last = value;
                last_time = time;
                print!(" (t={time:.2}, v={value:.4})");
            }
            println!();
            if axis == 'Z' {
                println!(
                    "  Z SWING: {first:.4} -> {last:.4} rad over {last_time:.2}s = {:.1} degrees",
                    last.to_degrees()
                );
            }
        }
        let num_trans = br.u32();
        let num_scale = br.u32();
        println!("  translation keys: {num_trans}, scale keys: {num_scale}");
        println!("  bytes consumed: {} / {}", br.pos, size);
    }
}
