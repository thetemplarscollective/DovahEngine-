//! Round-trip tests for the BSA reader, using synthetic archives built
//! in memory. No real game data is involved: the builder below writes
//! valid v104/v105 archives from the UESP spec, and the tests check the
//! reader lists and extracts them correctly.

use dovah_bsa::{BsaArchive, BSA_MAGIC, VERSION_LE, VERSION_SE};
use std::io::Cursor;

const FLAG_DIR_NAMES: u32 = 0x0001;
const FLAG_FILE_NAMES: u32 = 0x0002;
const FLAG_COMPRESSED: u32 = 0x0004;
const FLAG_EMBED_NAMES: u32 = 0x0100;
const SIZE_COMPRESSED_BIT: u32 = 0x4000_0000;

struct TestFile {
    name: &'static str,
    data: Vec<u8>,
    compressed: bool,
}

struct TestFolder {
    name: &'static str,
    files: Vec<TestFile>,
}

fn compress_payload(version: u32, data: &[u8]) -> Vec<u8> {
    if version == VERSION_SE {
        // v105 uses LZ4 *frame* format, not raw blocks (verified
        // against the real Textures0.bsa, 2026-11-13).
        use lz4_flex::frame::FrameEncoder;
        use std::io::Write;
        let mut enc = FrameEncoder::new(Vec::new());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    } else {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }
}

/// Build a minimal but spec-valid BSA archive in memory.
///
/// Layout: header, folder records, file-record blocks, file-name block,
/// file data. Folder-record offsets are stored as real offset +
/// totalFileNameLength, per spec. Hashes are zero (the reader never
/// validates them).
fn build_bsa(version: u32, flags: u32, folders: &[TestFolder]) -> Vec<u8> {
    let default_compressed = flags & FLAG_COMPRESSED != 0;
    let embed_names = flags & FLAG_EMBED_NAMES != 0;

    let file_count: usize = folders.iter().map(|f| f.files.len()).sum();
    let total_folder_name_len: usize = folders.iter().map(|f| f.name.len() + 1).sum();
    let total_file_name_len: usize = folders
        .iter()
        .flat_map(|f| f.files.iter())
        .map(|f| f.name.len() + 1)
        .sum();

    // Build the data blocks first, so file-record offsets are known.
    // Each block: [embedded bstring]? + [originalSize u32 + payload]?
    // or raw data.
    struct Block {
        bytes: Vec<u8>,
        stored_size: u32,
    }
    let mut blocks: Vec<Block> = Vec::with_capacity(file_count);
    for folder in folders {
        for file in &folder.files {
            let mut bytes = Vec::new();
            if embed_names {
                let full = format!("{}\\{}", folder.name, file.name);
                bytes.push((full.len() + 1) as u8);
                bytes.extend_from_slice(full.as_bytes());
                bytes.push(0);
            }
            if file.compressed {
                let payload = compress_payload(version, &file.data);
                bytes.extend_from_slice(&(file.data.len() as u32).to_le_bytes());
                bytes.extend_from_slice(&payload);
            } else {
                bytes.extend_from_slice(&file.data);
            }
            // Bit 30 is set when the file's state differs from the default.
            let mut stored_size = bytes.len() as u32;
            if file.compressed != default_compressed {
                stored_size |= SIZE_COMPRESSED_BIT;
            }
            blocks.push(Block { bytes, stored_size });
        }
    }

    let folder_record_len: usize = if version == VERSION_SE { 24 } else { 16 };
    let header_len = 36usize;
    let folder_records_len = folders.len() * folder_record_len;
    let blocks_start = header_len + folder_records_len;
    // Each file-record block: bzstring folder name + 16 bytes per file.
    let file_blocks_len: usize = folders
        .iter()
        .map(|f| 1 + f.name.len() + 1 + f.files.len() * 16)
        .sum();
    let name_block_start = blocks_start + file_blocks_len;
    let data_start = name_block_start + total_file_name_len;

    let mut out = Vec::new();

    // Header.
    out.extend_from_slice(BSA_MAGIC);
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&(header_len as u32).to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&(folders.len() as u32).to_le_bytes());
    out.extend_from_slice(&(file_count as u32).to_le_bytes());
    out.extend_from_slice(&(total_folder_name_len as u32).to_le_bytes());
    out.extend_from_slice(&(total_file_name_len as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // file flags
    out.extend_from_slice(&0u16.to_le_bytes()); // padding

    // Folder records.
    let mut block_offset = blocks_start;
    for folder in folders {
        out.extend_from_slice(&0u64.to_le_bytes()); // name hash
        out.extend_from_slice(&(folder.files.len() as u32).to_le_bytes());
        if version == VERSION_SE {
            out.extend_from_slice(&0u32.to_le_bytes()); // padding
        }
        // Stored offset includes totalFileNameLength.
        out.extend_from_slice(&((block_offset + total_file_name_len) as u32).to_le_bytes());
        if version == VERSION_SE {
            out.extend_from_slice(&0u32.to_le_bytes()); // padding
        }
        block_offset += 1 + folder.name.len() + 1 + folder.files.len() * 16;
    }

    // File-record blocks + data offsets.
    let mut data_offset = data_start;
    let mut block_idx = 0usize;
    for folder in folders {
        // bzstring folder name.
        out.push((folder.name.len() + 1) as u8);
        out.extend_from_slice(folder.name.as_bytes());
        out.push(0);
        for _ in &folder.files {
            let block = &blocks[block_idx];
            out.extend_from_slice(&0u64.to_le_bytes()); // file hash
            out.extend_from_slice(&block.stored_size.to_le_bytes());
            out.extend_from_slice(&(data_offset as u32).to_le_bytes());
            data_offset += block.bytes.len();
            block_idx += 1;
        }
    }

    // File-name block: root names, NUL-terminated, in record order.
    for folder in folders {
        for file in &folder.files {
            out.extend_from_slice(file.name.as_bytes());
            out.push(0);
        }
    }

    // File data.
    for block in &blocks {
        out.extend_from_slice(&block.bytes);
    }

    out
}

#[test]
fn v105_lists_and_extracts_mixed_compression() {
    let folders = vec![
        TestFolder {
            name: "textures",
            files: vec![
                TestFile {
                    name: "a.dds",
                    data: b"DDSFAKE-a".to_vec(),
                    compressed: false,
                },
                TestFile {
                    name: "b.dds",
                    data: b"lz4 round-trip payload, repeated. ".repeat(8),
                    compressed: true,
                },
            ],
        },
        TestFolder {
            name: "meshes",
            files: vec![TestFile {
                name: "c.nif",
                data: b"NIF-FAKE-c".to_vec(),
                compressed: false,
            }],
        },
    ];
    // Default-compressed archive with embedded names — the exact
    // combination the real Textures0.bsa uses. Uncompressed files
    // carry the flip bit.
    let bytes = build_bsa(
        VERSION_SE,
        FLAG_DIR_NAMES | FLAG_FILE_NAMES | FLAG_COMPRESSED | FLAG_EMBED_NAMES,
        &folders,
    );
    let mut archive = BsaArchive::from_reader(Cursor::new(bytes)).unwrap();
    assert_eq!(archive.version(), VERSION_SE);
    assert_eq!(archive.len(), 3);
    assert_eq!(
        archive.list(),
        vec!["meshes/c.nif", "textures/a.dds", "textures/b.dds"]
    );
    assert_eq!(
        archive.extract("textures/a.dds").unwrap(),
        b"DDSFAKE-a".to_vec()
    );
    assert_eq!(
        archive.extract("textures/b.dds").unwrap(),
        b"lz4 round-trip payload, repeated. ".repeat(8)
    );
    assert_eq!(
        archive.extract("meshes/c.nif").unwrap(),
        b"NIF-FAKE-c".to_vec()
    );
    assert!(archive.extract("textures/nope.dds").is_err());
}

#[test]
fn v104_zlib_with_embedded_names() {
    let folders = vec![TestFolder {
        name: "sound",
        files: vec![
            TestFile {
                name: "x.wav",
                data: b"WAVE-FAKE".to_vec(),
                compressed: false,
            },
            TestFile {
                name: "y.wav",
                data: b"zlib round-trip payload. ".repeat(10),
                compressed: true,
            },
        ],
    }];
    // Not compressed by default, embedded names on: the compressed file
    // carries the flip bit, and every block has a bstring prefix.
    let bytes = build_bsa(
        VERSION_LE,
        FLAG_DIR_NAMES | FLAG_FILE_NAMES | FLAG_EMBED_NAMES,
        &folders,
    );
    let mut archive = BsaArchive::from_reader(Cursor::new(bytes)).unwrap();
    assert_eq!(archive.version(), VERSION_LE);
    assert_eq!(archive.list(), vec!["sound/x.wav", "sound/y.wav"]);
    assert_eq!(
        archive.extract("sound/x.wav").unwrap(),
        b"WAVE-FAKE".to_vec()
    );
    assert_eq!(
        archive.extract("sound/y.wav").unwrap(),
        b"zlib round-trip payload. ".repeat(10)
    );
}

#[test]
fn rejects_bad_magic() {
    let mut bytes = build_bsa(
        VERSION_SE,
        FLAG_DIR_NAMES | FLAG_FILE_NAMES,
        &[TestFolder {
            name: "a",
            files: vec![TestFile {
                name: "b.txt",
                data: b"hi".to_vec(),
                compressed: false,
            }],
        }],
    );
    bytes[0] = b'X';
    assert!(BsaArchive::from_reader(Cursor::new(bytes)).is_err());
}

#[test]
fn rejects_unsupported_version() {
    let mut bytes = build_bsa(
        VERSION_SE,
        FLAG_DIR_NAMES | FLAG_FILE_NAMES,
        &[TestFolder {
            name: "a",
            files: vec![TestFile {
                name: "b.txt",
                data: b"hi".to_vec(),
                compressed: false,
            }],
        }],
    );
    // Patch the version field to 103 (Oblivion).
    bytes[4..8].copy_from_slice(&103u32.to_le_bytes());
    assert!(BsaArchive::from_reader(Cursor::new(bytes)).is_err());
}
