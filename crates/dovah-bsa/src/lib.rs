//! BSA archive reader for Dovahengine.
//!
//! Reads Bethesda Softworks Archive files, versions 104 (Skyrim LE) and
//! 105 (Skyrim Special Edition), following the public UESP format
//! documentation (`docs/formats/bsa.md`). This crate contains no Bethesda
//! code and ships no game data: it parses archives the user already owns.
//!
//! The reader is streaming-friendly: it parses the small directory
//! structures up front and seeks to file data on demand, so opening a
//! multi-gigabyte archive does not load it into memory.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

pub const BSA_MAGIC: &[u8; 4] = b"BSA\0";
pub const VERSION_LE: u32 = 104;
pub const VERSION_SE: u32 = 105;

const FLAG_DIR_NAMES: u32 = 0x0001;
const FLAG_FILE_NAMES: u32 = 0x0002;
const FLAG_COMPRESSED: u32 = 0x0004;
const FLAG_XBOX: u32 = 0x0040;
const FLAG_EMBED_NAMES: u32 = 0x0100;

const SIZE_COMPRESSED_BIT: u32 = 0x4000_0000;
const SIZE_MASK: u32 = 0x3FFF_FFFF;

const HEADER_LEN: usize = 36;

#[derive(Debug)]
pub enum BsaError {
    Io(io::Error),
    BadMagic,
    UnsupportedVersion(u32),
    XboxArchive,
    Truncated(&'static str),
    NotFound(String),
    Decompress(&'static str),
    InvalidData(&'static str),
}

impl std::fmt::Display for BsaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BsaError::Io(e) => write!(f, "I/O error: {e}"),
            BsaError::BadMagic => write!(f, "not a BSA archive (bad magic)"),
            BsaError::UnsupportedVersion(v) => {
                write!(f, "unsupported BSA version {v} (want 104 or 105)")
            }
            BsaError::XboxArchive => {
                write!(f, "Xbox 360 BSA archives (big-endian) are not supported")
            }
            BsaError::Truncated(what) => write!(f, "truncated BSA file in {what}"),
            BsaError::NotFound(path) => write!(f, "file not in archive: {path}"),
            BsaError::Decompress(codec) => {
                write!(f, "failed to decompress file data ({codec})")
            }
            BsaError::InvalidData(what) => write!(f, "invalid BSA data: {what}"),
        }
    }
}

impl std::error::Error for BsaError {}

impl From<io::Error> for BsaError {
    fn from(e: io::Error) -> Self {
        BsaError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, BsaError>;

/// Turn an unexpected-EOF read failure into a `Truncated` error; pass
/// other I/O errors through unchanged.
fn map_eof(e: io::Error, what: &'static str) -> BsaError {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        BsaError::Truncated(what)
    } else {
        BsaError::Io(e)
    }
}

/// One parsed file record: (stored size, absolute offset, compressed).
type RawFileRecord = (u32, u64, bool);

#[derive(Debug, Clone)]
struct FileEntry {
    /// Stored block size (flag bit stripped). Covers the whole data
    /// block, including any embedded name and the 4-byte original-size
    /// prefix of a compressed file.
    size: u32,
    /// Absolute offset of the data block from the start of the file.
    offset: u64,
    compressed: bool,
}

/// An open BSA archive.
///
/// The index is built at open time from the directory structures; file
/// contents are read (and decompressed) on demand by [`BsaArchive::extract`].
#[derive(Debug)]
pub struct BsaArchive<R: Read + Seek> {
    reader: R,
    version: u32,
    archive_flags: u32,
    files: BTreeMap<String, FileEntry>,
}

impl BsaArchive<File> {
    /// Open a BSA archive from a file on disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        Self::from_reader(file)
    }
}

impl<R: Read + Seek> BsaArchive<R> {
    /// Open a BSA archive from any readable, seekable source.
    pub fn from_reader(mut reader: R) -> Result<Self> {
        let mut header = [0u8; HEADER_LEN];
        reader
            .read_exact(&mut header)
            .map_err(|e| map_eof(e, "header"))?;
        if header[0..4] != *BSA_MAGIC {
            return Err(BsaError::BadMagic);
        }
        let version = u32::from_le_bytes(header[4..8].try_into().unwrap());
        if version != VERSION_LE && version != VERSION_SE {
            return Err(BsaError::UnsupportedVersion(version));
        }
        let archive_flags = u32::from_le_bytes(header[12..16].try_into().unwrap());
        if archive_flags & FLAG_XBOX != 0 {
            return Err(BsaError::XboxArchive);
        }
        let folder_count = u32::from_le_bytes(header[16..20].try_into().unwrap()) as usize;
        let file_count = u32::from_le_bytes(header[20..24].try_into().unwrap()) as usize;
        let total_file_name_len = u32::from_le_bytes(header[28..32].try_into().unwrap()) as u64;
        let has_dir_names = archive_flags & FLAG_DIR_NAMES != 0;
        let has_file_names = archive_flags & FLAG_FILE_NAMES != 0;
        let default_compressed = archive_flags & FLAG_COMPRESSED != 0;

        // Folder records: 16 bytes each on v104, 24 on v105.
        let folder_record_len: usize = if version == VERSION_SE { 24 } else { 16 };
        let mut folders: Vec<(u32, u64)> = Vec::with_capacity(folder_count);
        for _ in 0..folder_count {
            let mut rec = vec![0u8; folder_record_len];
            reader
                .read_exact(&mut rec)
                .map_err(|e| map_eof(e, "folder record"))?;
            let count = u32::from_le_bytes(rec[8..12].try_into().unwrap());
            let stored_offset = if version == VERSION_SE {
                u32::from_le_bytes(rec[16..20].try_into().unwrap()) as u64
            } else {
                u32::from_le_bytes(rec[12..16].try_into().unwrap()) as u64
            };
            // Stored offsets include totalFileNameLength; subtract it to
            // get the real offset of the file-record block.
            let block_offset = stored_offset
                .checked_sub(total_file_name_len)
                .ok_or(BsaError::InvalidData("folder record offset"))?;
            folders.push((count, block_offset));
        }

        // File-record blocks: folder name (bzstring) + file records.
        // Collect first; names are resolved from the name block below.
        let mut folders_data: Vec<(String, Vec<RawFileRecord>)> = Vec::with_capacity(folder_count);
        let mut name_block_start: u64 = 0;
        for (count, block_offset) in &folders {
            reader.seek(SeekFrom::Start(*block_offset))?;
            let folder_name = if has_dir_names {
                read_bzstring(&mut reader)?
            } else {
                String::new()
            };
            let mut records = Vec::with_capacity(*count as usize);
            for _ in 0..*count {
                let mut rec = [0u8; 16];
                reader
                    .read_exact(&mut rec)
                    .map_err(|e| map_eof(e, "file record"))?;
                let raw_size = u32::from_le_bytes(rec[8..12].try_into().unwrap());
                let offset = u32::from_le_bytes(rec[12..16].try_into().unwrap()) as u64;
                // Bit 30 flips the compression state relative to the
                // archive default.
                let compressed = if raw_size & SIZE_COMPRESSED_BIT != 0 {
                    !default_compressed
                } else {
                    default_compressed
                };
                records.push((raw_size & SIZE_MASK, offset, compressed));
            }
            let end = reader.stream_position()?;
            if end > name_block_start {
                name_block_start = end;
            }
            folders_data.push((folder_name, records));
        }

        // File-name block: root names only, NUL-terminated, in
        // file-record order across all folders.
        let mut names: Vec<String> = Vec::with_capacity(file_count);
        if has_file_names {
            reader.seek(SeekFrom::Start(name_block_start))?;
            let mut buf = vec![0u8; total_file_name_len as usize];
            reader
                .read_exact(&mut buf)
                .map_err(|e| map_eof(e, "file name block"))?;
            for part in buf.split(|b| *b == 0) {
                if part.is_empty() {
                    continue;
                }
                names.push(String::from_utf8_lossy(part).into_owned());
            }
        }

        let mut name_iter = names.into_iter();
        let mut files = BTreeMap::new();
        for (folder_name, records) in folders_data {
            for (size, offset, compressed) in records {
                let file_name = if has_file_names {
                    name_iter
                        .next()
                        .ok_or(BsaError::Truncated("file name block"))?
                } else {
                    format!("unnamed_{offset}")
                };
                let path = if folder_name.is_empty() {
                    file_name
                } else {
                    format!("{}/{}", folder_name.replace('\\', "/"), file_name)
                };
                files.insert(
                    path,
                    FileEntry {
                        size,
                        offset,
                        compressed,
                    },
                );
            }
        }

        Ok(Self {
            reader,
            version,
            archive_flags,
            files,
        })
    }

    /// BSA version of this archive (104 or 105).
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Number of files in the archive.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the archive contains no files.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Full paths of every file, e.g. `textures/sky/clouds.dds`.
    /// Paths use `/` separators (archive folder names use `\`).
    pub fn list(&self) -> Vec<&str> {
        self.files.keys().map(|s| s.as_str()).collect()
    }

    /// Whether the archive contains a file at `path`.
    pub fn contains(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    /// Read and (if needed) decompress one file by its full path.
    pub fn extract(&mut self, path: &str) -> Result<Vec<u8>> {
        let entry = self
            .files
            .get(path)
            .ok_or_else(|| BsaError::NotFound(path.to_string()))?
            .clone();
        self.reader.seek(SeekFrom::Start(entry.offset))?;
        let mut block = vec![0u8; entry.size as usize];
        self.reader
            .read_exact(&mut block)
            .map_err(|e| map_eof(e, "file data"))?;
        let mut data: &[u8] = &block;

        // Embedded file name (flag 0x100): bstring, length byte counts
        // the trailing NUL.
        if self.archive_flags & FLAG_EMBED_NAMES != 0 {
            if data.is_empty() {
                return Err(BsaError::Truncated("embedded file name"));
            }
            let len = data[0] as usize;
            if data.len() < 1 + len {
                return Err(BsaError::Truncated("embedded file name"));
            }
            data = &data[1 + len..];
        }

        if !entry.compressed {
            return Ok(data.to_vec());
        }
        if data.len() < 4 {
            return Err(BsaError::Truncated("compressed size prefix"));
        }
        let original_size = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
        let payload = &data[4..];
        match self.version {
            // v105 uses the LZ4 *frame* format (magic 04 22 4D 18),
            // not raw LZ4 blocks — verified against the real
            // Skyrim - Textures0.bsa (2026-11-13), where block
            // decoding failed on all 5,889 files. The frame's own
            // output must match the u32 size prefix exactly.
            VERSION_SE => {
                let mut decoder = lz4_flex::frame::FrameDecoder::new(payload);
                let mut out = Vec::with_capacity(original_size);
                decoder
                    .read_to_end(&mut out)
                    .map_err(|_| BsaError::Decompress("LZ4"))?;
                if out.len() != original_size {
                    return Err(BsaError::Decompress("LZ4"));
                }
                Ok(out)
            }
            _ => {
                let mut decoder = flate2::read::ZlibDecoder::new(payload);
                let mut out = Vec::with_capacity(original_size);
                decoder
                    .read_to_end(&mut out)
                    .map_err(|_| BsaError::Decompress("zlib"))?;
                Ok(out)
            }
        }
    }
}

/// Read a `bzstring`: one length byte (counting the trailing NUL),
/// then the string bytes plus NUL.
fn read_bzstring<R: Read>(reader: &mut R) -> Result<String> {
    let mut len_buf = [0u8; 1];
    reader
        .read_exact(&mut len_buf)
        .map_err(|e| map_eof(e, "folder name length"))?;
    let len = len_buf[0] as usize;
    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .map_err(|e| map_eof(e, "folder name"))?;
    if buf.last() == Some(&0) {
        buf.pop();
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}
