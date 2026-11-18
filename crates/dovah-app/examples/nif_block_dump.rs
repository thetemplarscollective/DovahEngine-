//! Dump the RAW bytes of NIF blocks whose type name contains a
//! substring — for reverse-engineering block layouts that
//! dovah-nif keeps as Unknown (e.g. BSEffectShaderProperty).
//! Walks the NIF header manually (block types + sizes) to find
//! each block's byte range, then hex-dumps it.
//!
//! Usage: cargo run -p dovah-app --example nif_block_dump --
//!        <nif-path> <type-substring>
//! e.g. meshes\\effects\\fxwaterfallskirttallfront.nif EffectShader

use dovah_bsa::BsaArchive;
use std::collections::HashMap;
use std::fs::{self, File};

fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

struct R<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> R<'a> {
    fn u8(&mut self) -> u8 {
        let v = self.b[self.pos];
        self.pos += 1;
        v
    }
    fn u16(&mut self) -> u16 {
        let v = u16::from_le_bytes(self.b[self.pos..self.pos + 2].try_into().unwrap());
        self.pos += 2;
        v
    }
    fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes(self.b[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        v
    }
    fn line(&mut self) -> String {
        let start = self.pos;
        while self.b[self.pos] != b'\n' {
            self.pos += 1;
        }
        let s = String::from_utf8_lossy(&self.b[start..self.pos]).into_owned();
        self.pos += 1;
        s
    }
    fn sized_string(&mut self) -> String {
        let len = self.u32() as usize;
        let s = String::from_utf8_lossy(&self.b[self.pos..self.pos + len]).into_owned();
        self.pos += len;
        s
    }
    fn export_string(&mut self) -> String {
        let len = self.u8() as usize;
        let mut s = String::from_utf8_lossy(&self.b[self.pos..self.pos + len]).into_owned();
        self.pos += len;
        if s.ends_with('\0') {
            s.pop();
        }
        s
    }
}

fn main() {
    let want = std::env::args().nth(1).expect("NIF path argument");
    let needle = std::env::args()
        .nth(2)
        .expect("type substring argument")
        .to_lowercase();
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
        if let Ok(archive) = BsaArchive::open(&path) {
            let idx = archives.len();
            for stored in archive.list() {
                index.entry(normalize(stored)).or_insert(idx);
            }
            archives.push(archive);
        }
    }
    let idx = *index.get(&normalize(&want)).expect("NIF not in BSAs");
    let stored: String = archives[idx]
        .list()
        .into_iter()
        .find(|s| normalize(s) == normalize(&want))
        .unwrap()
        .to_string();
    let bytes = archives[idx].extract(&stored).expect("extract NIF");
    let mut r = R { b: &bytes, pos: 0 };
    let header = r.line();
    assert!(header.starts_with("Gamebryo File Format"));
    let _version = r.u32();
    let _endian = r.u8();
    let _user_version = r.u32();
    let num_blocks = r.u32() as usize;
    let bs_version = r.u32();
    let _author = r.export_string();
    if bs_version < 131 {
        let _process = r.export_string();
    }
    let _export = r.export_string();
    if bs_version >= 103 {
        let _max = r.export_string();
    }
    let num_types = r.u16() as usize;
    let mut types = Vec::with_capacity(num_types);
    for _ in 0..num_types {
        types.push(r.sized_string());
    }
    let mut type_idx = Vec::with_capacity(num_blocks);
    for _ in 0..num_blocks {
        type_idx.push(r.u16() as usize);
    }
    let mut sizes = Vec::with_capacity(num_blocks);
    for _ in 0..num_blocks {
        sizes.push(r.u32() as usize);
    }
    let num_strings = r.u32() as usize;
    let _max_len = r.u32();
    let mut strings = Vec::with_capacity(num_strings);
    for _ in 0..num_strings {
        strings.push(r.sized_string());
    }
    let num_groups = r.u32() as usize;
    for _ in 0..num_groups {
        let _g = r.u32();
    }
    let data_start = r.pos;
    println!("{want}: {num_blocks} blocks, string table:");
    for (i, s) in strings.iter().enumerate() {
        println!("  str[{i}] = {s:?}");
    }
    let mut off = data_start;
    for i in 0..num_blocks {
        let size = sizes[i];
        let ty = &types[type_idx[i]];
        if ty.to_lowercase().contains(&needle) {
            let block = &bytes[off..off + size];
            println!("--- block [{i}] {ty} ({size} bytes) ---");
            for (j, chunk) in block.chunks(16).enumerate() {
                let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
                let ascii: String = chunk
                    .iter()
                    .map(|&b| {
                        if (0x20..0x7f).contains(&b) {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                println!("  {:04x}: {:<47} |{ascii}|", j * 16, hex.join(" "));
            }
            // Also try decoding as floats from offset 0.
            let floats: Vec<f32> = block
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect();
            println!("  as f32: {floats:?}");
        }
        off += size;
    }
}
