//! Secret-door probe: decode the controller linkage of
//! RifRmSmSecretCabinetDoor01.nif by hand.
//!
//! The NIF carries NiControllerSequence blocks (one "Open", one
//! "Close") whose controlled blocks name their target node
//! (LeftDoor01 / RightDoor01) and point at a NiTransformInterpolator
//! -> NiTransformData. This probe parses the NIF header itself
//! (like door_anim.rs), dumps the raw bytes of the sequence and
//! interpolator blocks, and decodes them against the nif.xml
//! layouts to prove which Open angle drives which panel.
//!
//! Usage: cargo run -p dovah-app --example secret_door_probe
//! (hardcoded to the secret door NIF; the farmhouse doors are
//! dumped too for comparison).

use dovah_bsa::BsaArchive;
use std::collections::HashMap;
use std::fs::{self, File};

const NIFS: [&str; 4] = [
    "meshes\\Architecture\\Riften\\Interior\\SmallRoomFirst\\RifRmSmSecretCabinetDoor01.nif",
    "meshes\\Architecture\\Farmhouse\\FarmhouseLDoor01.nif",
    "meshes\\Architecture\\Farmhouse\\FarmhouseAnimDoor01.nif",
    "meshes\\Architecture\\Farmhouse\\FarmhouseAnimDoor02.nif",
];

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
    fn i32(&mut self) -> i32 {
        self.u32() as i32
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
        self.pos += 1;
        s
    }
    fn sized_string(&mut self) -> String {
        let len = self.u32() as usize;
        let s = String::from_utf8_lossy(&self.bytes[self.pos..self.pos + len]).to_string();
        self.pos += len;
        s
    }
    fn export_string(&mut self) -> String {
        let len = self.u8() as usize;
        let mut s = String::from_utf8_lossy(&self.bytes[self.pos..self.pos + len]).to_string();
        self.pos += len;
        if s.ends_with('\0') {
            s.pop();
        }
        s
    }
}

/// Parse a NiTransformData block's Z-key delta (same rule as
/// doors.rs open_angle_from_nif): first -> last Z value.
fn z_delta(bytes: &[u8]) -> Option<(f32, f32)> {
    let mut br = Reader { bytes, pos: 0 };
    let _num_rot = br.u32();
    let rot_type = br.u32();
    if rot_type != 4 {
        return None;
    }
    let mut z = None;
    for _axis in 0..3 {
        let n = br.u32() as usize;
        let _t = br.u32();
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
        }
        z = Some((first, last, last_time));
    }
    let (first, last, last_time) = z?;
    Some((last - first, last_time))
}

fn main() {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut archives: Vec<BsaArchive<File>> = Vec::new();
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

    for nif_path in NIFS {
        let idx = *index.get(&normalize(nif_path)).expect("NIF in BSA index");
        let stored: String = archives[idx]
            .list()
            .into_iter()
            .find(|s| normalize(s) == normalize(nif_path))
            .expect("stored path")
            .to_string();
        let bytes = archives[idx].extract(&stored).expect("extract NIF");
        println!("\n===== {nif_path} ({} bytes) =====", bytes.len());

        // The dovah-nif parser's answer (ControllerSequence +
        // TransformInterpolator + open_angles_by_node): this must
        // agree with the hand-decoded hex below.
        match dovah_nif::NifFile::from_bytes(&bytes) {
            Ok(nif) => {
                for block in &nif.blocks {
                    if let dovah_nif::Block::ControllerSequence(seq) = block {
                        println!(
                            "  dovah-nif sequence {:?}: {:?}",
                            seq.name,
                            seq.controlled
                                .iter()
                                .map(|c| (c.target_name.as_str(), c.interpolator))
                                .collect::<Vec<_>>()
                        );
                    }
                }
                let mut angles: Vec<_> = nif.open_angles_by_node().into_iter().collect();
                angles.sort_by(|a, b| a.0.cmp(&b.0));
                for (node, (angle, duration)) in angles {
                    println!(
                        "  dovah-nif open angle: {node} = {angle:.4} rad ({:.1} deg) over {duration:.2}s",
                        angle.to_degrees()
                    );
                }
            }
            Err(e) => println!("  dovah-nif parse FAILED: {e}"),
        }

        let mut r = Reader {
            bytes: &bytes,
            pos: 0,
        };
        let _header = r.line();
        let _version = r.u32();
        let _endian = r.u8();
        let _user_version = r.u32();
        let num_blocks = r.u32() as usize;
        let _bs_version = r.u32();
        let _author = r.export_string();
        let _process = r.export_string();
        let _export = r.export_string();
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
        let num_groups = r.u32();
        for _ in 0..num_groups {
            let _ = r.u32();
        }
        let blocks_start = r.pos;
        let mut offsets = Vec::with_capacity(num_blocks);
        let mut off = blocks_start;
        for size in &block_sizes {
            offsets.push(off);
            off += size;
        }
        let block_bytes = |i: usize| -> &[u8] { &bytes[offsets[i]..offsets[i] + block_sizes[i]] };
        let s = |idx: u32| -> &str {
            strings
                .get(idx as usize)
                .map(|s| s.as_str())
                .unwrap_or("<?>")
        };
        println!("  string table ({} strings):", strings.len());
        for (i, st) in strings.iter().enumerate() {
            println!("    [{i}] {st:?}");
        }
        // Hex dump of the controller blocks [2] [3] for layout
        // comparison.
        for i in [2usize, 3] {
            let b = block_bytes(i);
            println!(
                "  [{i}] {} size={} HEX:",
                block_types[block_type_index[i]],
                b.len()
            );
            for (row, chunk) in b.chunks(16).enumerate() {
                let hex: Vec<String> = chunk.iter().map(|x| format!("{x:02x}")).collect();
                println!("    {:04x}: {}", row * 16, hex.join(" "));
            }
        }

        // NiTransformData Z deltas, for resolving interpolators.
        let mut data_delta: HashMap<usize, (f32, f32)> = HashMap::new();
        for (i, &ti) in block_type_index.iter().enumerate() {
            if block_types[ti] == "NiTransformData" {
                if let Some(d) = z_delta(block_bytes(i)) {
                    println!(
                        "  [{i}] NiTransformData z_delta={:.4} rad ({:.1} deg) over {:.2}s",
                        d.0,
                        d.0.to_degrees(),
                        d.1
                    );
                    data_delta.insert(i, d);
                }
            }
        }
        // NiDefaultAVObjectPalette: name -> object refs. Dump it
        // raw: u32 numObjs, then per obj (u32 string idx, i32 ref),
        // then a trailing u32.
        for (i, &ti) in block_type_index.iter().enumerate() {
            if block_types[ti] != "NiDefaultAVObjectPalette" {
                continue;
            }
            let b = block_bytes(i);
            let mut br = Reader { bytes: b, pos: 0 };
            let num = br.u32();
            println!(
                "  [{i}] NiDefaultAVObjectPalette size={} numObjs={num}",
                b.len()
            );
            for _ in 0..num {
                let name_idx = br.u32();
                let obj = br.i32();
                println!("      obj {:?} -> block {obj}", s(name_idx));
            }
            if br.pos + 4 <= b.len() {
                println!("      trailing u32: {}", br.u32());
            }
            println!("      consumed={}/{}", br.pos, b.len());
        }

        // NiTransformInterpolator: translation (3f) + rotation
        // quat (4f) + scale (1f) + data ref (i32) = 36 bytes.
        let mut interp_data: HashMap<usize, i32> = HashMap::new();
        for (i, &ti) in block_type_index.iter().enumerate() {
            if block_types[ti] != "NiTransformInterpolator" {
                continue;
            }
            let b = block_bytes(i);
            let mut br = Reader { bytes: b, pos: 0 };
            let tx = br.f32();
            let ty = br.f32();
            let tz = br.f32();
            let qw = br.f32();
            let qx = br.f32();
            let qy = br.f32();
            let qz = br.f32();
            let scale = br.f32();
            let data = br.i32();
            println!(
                "  [{i}] NiTransformInterpolator size={} t=({tx:.2},{ty:.2},{tz:.2}) quat=({qw:.3},{qx:.3},{qy:.3},{qz:.3}) scale={scale:.3} data_ref={data} consumed={}/{}",
                b.len(),
                br.pos,
                b.len()
            );
            interp_data.insert(i, data);
        }

        // Raw hex dump of ALL NiControllerSequence blocks, to decode
        // their layout by hand.
        for (i, &ti) in block_type_index.iter().enumerate() {
            if block_types[ti] != "NiControllerSequence" {
                continue;
            }
            let b = block_bytes(i);
            println!("  [{i}] NiControllerSequence size={} HEX:", b.len());
            for (row, chunk) in b.chunks(16).enumerate() {
                let hex: Vec<String> = chunk.iter().map(|x| format!("{x:02x}")).collect();
                println!("    {:04x}: {}", row * 16, hex.join(" "));
            }
        }

        // Structured decode is disabled until the layout is proven
        // from the hex dumps above.
        let _ = (&interp_data, &data_delta, &s);
    }
}
