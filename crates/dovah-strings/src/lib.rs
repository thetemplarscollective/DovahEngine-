//! Skyrim SE localized strings (`.STRINGS` / `.DLSTRINGS` /
//! `.ILSTRINGS`) reader for Dovahengine.
//!
//! Skyrim.esm is localized: text fields in records (`FULL`,
//! `DESC`, INFO `NAM1`, ...) hold a 32-bit string ID instead of
//! text, and the text lives in these files. There is one file set
//! per plugin (`Skyrim_English.*`, `Update_English.*`, ...) and
//! three kinds per set:
//!
//! * `.STRINGS` — general strings: NPC/item/location names,
//!   descriptions, topic text.
//! * `.ILSTRINGS` — dialogue lines (INFO `NAM1` responses) and
//!   interface strings. Proven from Dylan's real files (M18):
//!   all 11 NAM1s tested resolved in `.ILSTRINGS` and NONE in
//!   `.DLSTRINGS` (Nexus 1842 and Patch.bsa agree). The
//!   remembered docs that say NAM1 -> `.DLSTRINGS` are WRONG
//!   for Skyrim SE — do not "fix" this back.
//! * `.DLSTRINGS` — long-form text (book text with font markup,
//!   loading tips, descriptions). Not loaded in M18.
//!
//! Format (proven against the real files in
//! `test-data/strings/`, Nexus mod 1842, and the same files
//! inside `Skyrim_-_Patch.bsa`):
//!
//! ```text
//! u32 count              number of directory entries
//! u32 data_size          bytes in the string data section
//! count × { u32 id, u32 offset }   directory, sorted by id
//! string data            data_size bytes
//! ```
//!
//! The directory offset is relative to the start of the data
//! section (i.e. to byte `8 + count * 8` of the file). In
//! `.STRINGS` the string at the offset is plain NUL-terminated
//! text. In `.DLSTRINGS` and `.ILSTRINGS` each string is
//! prefixed by its own length as a u32 (length INCLUDES the
//! NUL terminator), and the text starts 4 bytes past the
//! offset — the first DLSTRINGS entry reads `18 01 00 00`
//! (280) then 279 text bytes + NUL, and the next entry sits
//! exactly 284 bytes on. A reader that ignores the prefix
//! decodes garbage (the length bytes as text); a reader that
//! applies it to `.STRINGS` skips the first four letters of
//! every name. The kind is therefore part of the parse call,
//! not guessed.
//!
//! String IDs are the directory IDs verbatim: the u32 in the
//! record IS the ID in the file (Delphine's NPC_ `FULL` is the
//! ID whose `.STRINGS` entry reads "Delphine" — see the
//! `dialogue_probe` example). No masking, no high-byte file
//! marker: which file to look in is decided by the FIELD
//! (`FULL` -> `.STRINGS`, INFO `NAM1` -> `.ILSTRINGS`), never
//! by the ID itself.

use std::collections::HashMap;

/// Which kind of strings file this is — the kind decides whether
/// each string carries a u32 length prefix (see the module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringsKind {
    /// `.STRINGS`: plain NUL-terminated text, no prefix.
    General,
    /// `.DLSTRINGS` / `.ILSTRINGS`: u32 length prefix (including
    /// the NUL) before the text.
    LengthPrefixed,
}

#[derive(Debug)]
pub enum StringsError {
    Truncated(&'static str),
    InvalidData(&'static str),
}

impl std::fmt::Display for StringsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StringsError::Truncated(what) => write!(f, "truncated strings file in {what}"),
            StringsError::InvalidData(what) => write!(f, "invalid strings data: {what}"),
        }
    }
}

impl std::error::Error for StringsError {}

pub type Result<T> = std::result::Result<T, StringsError>;

/// One parsed strings file: a map from string ID to text.
///
/// The map is built eagerly at parse time (the largest file,
/// `Skyrim_English.ILSTRINGS`, holds 34,427 strings in 2.4 MB
/// of text — small enough to keep decoded). Lookups are plain
/// hash lookups; a missing ID is `None`, never a panic — a
/// record can reference a string the loaded files do not have
/// (a newer game version than the strings set, a DLC string in
/// the wrong file), and the caller falls back to the EDID.
#[derive(Debug, Default)]
pub struct StringsFile {
    strings: HashMap<u32, String>,
}

impl StringsFile {
    /// Parse a strings file from its bytes. `kind` selects the
    /// per-string layout (see [`StringsKind`]).
    pub fn from_bytes(bytes: &[u8], kind: StringsKind) -> Result<Self> {
        if bytes.len() < 8 {
            return Err(StringsError::Truncated("header"));
        }
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let data_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let dir_bytes = count
            .checked_mul(8)
            .ok_or(StringsError::InvalidData("directory size overflow"))?;
        let data_start = 8usize
            .checked_add(dir_bytes)
            .ok_or(StringsError::InvalidData("directory size overflow"))?;
        if bytes.len() < data_start + data_size {
            return Err(StringsError::Truncated("string data"));
        }
        let data = &bytes[data_start..data_start + data_size];
        let mut strings = HashMap::with_capacity(count);
        for i in 0..count {
            let base = 8 + i * 8;
            let id = u32::from_le_bytes(bytes[base..base + 4].try_into().unwrap());
            let offset = u32::from_le_bytes(bytes[base + 4..base + 8].try_into().unwrap()) as usize;
            let text = match kind {
                StringsKind::General => read_zstring(data, offset),
                StringsKind::LengthPrefixed => {
                    // u32 length (including the NUL) then the
                    // text. The length is trusted only as far as
                    // the data section allows; the text is still
                    // read up to its NUL so a corrupt length
                    // cannot run the read past the section.
                    let Some(len_bytes) = data.get(offset..offset + 4) else {
                        continue;
                    };
                    let len = u32::from_le_bytes(len_bytes.try_into().unwrap()) as usize;
                    if len == 0 {
                        continue;
                    }
                    let text_start = offset + 4;
                    let text_end = text_start.saturating_add(len.saturating_sub(1));
                    let Some(slice) = data.get(text_start..text_end.min(data.len())) else {
                        continue;
                    };
                    let end = slice.iter().position(|b| *b == 0).unwrap_or(slice.len());
                    String::from_utf8_lossy(&slice[..end]).into_owned()
                }
            };
            strings.insert(id, text);
        }
        Ok(StringsFile { strings })
    }

    /// Look up a string by ID. `None` when the file has no entry
    /// for the ID.
    pub fn get(&self, id: u32) -> Option<&str> {
        self.strings.get(&id).map(|s| s.as_str())
    }

    /// Number of strings in the file.
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// Whether the file holds no strings.
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}

/// Read a NUL-terminated string at `offset` within `data`.
/// Returns an empty string when the offset is out of range (a
/// corrupt directory entry degrades to an empty string, not a
/// panic — the caller treats it as a miss).
fn read_zstring(data: &[u8], offset: usize) -> String {
    let Some(slice) = data.get(offset..) else {
        return String::new();
    };
    let end = slice.iter().position(|b| *b == 0).unwrap_or(slice.len());
    String::from_utf8_lossy(&slice[..end]).into_owned()
}

/// A set of strings files for one plugin: the general
/// (`.STRINGS`), dialogue (`.ILSTRINGS`), and long-form
/// (`.DLSTRINGS`) files. The long-form file holds book
/// descriptions (BOOK `CNAM`), book text, and loading tips —
/// loaded since the inventory redesign (book descriptions show
/// in the detail pane).
///
/// Lookups take the plugin's file set: a Skyrim.esm record's
/// `FULL` resolves in the Skyrim set, a Dragonborn.esm record's
/// in the Dragonborn set. The caller picks the set by the
/// record's plugin (the form ID's top byte is the plugin index
/// in load order).
#[derive(Debug, Default)]
pub struct StringsSet {
    pub general: StringsFile,
    pub dialogue: StringsFile,
    pub longform: StringsFile,
}

impl StringsSet {
    /// Look up a general string (a `FULL`/`DESC` field) by ID.
    pub fn general(&self, id: u32) -> Option<&str> {
        self.general.get(id)
    }

    /// Look up a dialogue line (an INFO `NAM1` field) by ID.
    pub fn dialogue(&self, id: u32) -> Option<&str> {
        self.dialogue.get(id)
    }

    /// Look up a long-form string (a BOOK `CNAM` description)
    /// by ID.
    pub fn longform(&self, id: u32) -> Option<&str> {
        self.longform.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic strings file: header + directory +
    /// data, with the given (id, text) entries. `length_prefix`
    /// selects the DLSTRINGS/ILSTRINGS layout.
    fn build_file(entries: &[(u32, &str)], length_prefix: bool) -> Vec<u8> {
        let mut data = Vec::new();
        let mut dir = Vec::new();
        for (id, text) in entries {
            dir.push((*id, data.len() as u32));
            if length_prefix {
                let len = text.len() + 1;
                data.extend_from_slice(&(len as u32).to_le_bytes());
            }
            data.extend_from_slice(text.as_bytes());
            data.push(0);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        for (id, offset) in dir {
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
        }
        out.extend_from_slice(&data);
        out
    }

    #[test]
    fn general_file_round_trips() {
        let bytes = build_file(
            &[(1, "Delphine"), (2, "Iron Sword"), (7, "Riverwood")],
            false,
        );
        let file = StringsFile::from_bytes(&bytes, StringsKind::General).unwrap();
        assert_eq!(file.len(), 3);
        assert_eq!(file.get(1), Some("Delphine"));
        assert_eq!(file.get(2), Some("Iron Sword"));
        assert_eq!(file.get(7), Some("Riverwood"));
        assert_eq!(file.get(3), None);
    }

    #[test]
    fn length_prefixed_file_round_trips() {
        let bytes = build_file(
            &[(0x44, "What do you want?"), (0x5c, "I don't trust you.")],
            true,
        );
        let file = StringsFile::from_bytes(&bytes, StringsKind::LengthPrefixed).unwrap();
        assert_eq!(file.len(), 2);
        assert_eq!(file.get(0x44), Some("What do you want?"));
        assert_eq!(file.get(0x5c), Some("I don't trust you."));
        assert_eq!(file.get(0x45), None);
    }

    #[test]
    fn general_kind_ignores_no_prefix() {
        // A length-prefixed file parsed as General decodes the
        // length bytes as text — the kinds are not
        // interchangeable, which is why the kind is explicit.
        let bytes = build_file(&[(1, "Delphine")], true);
        let file = StringsFile::from_bytes(&bytes, StringsKind::General).unwrap();
        assert_ne!(file.get(1), Some("Delphine"));
    }

    #[test]
    fn truncated_header_is_an_error() {
        assert!(matches!(
            StringsFile::from_bytes(&[1, 0, 0], StringsKind::General),
            Err(StringsError::Truncated(_))
        ));
    }

    #[test]
    fn truncated_data_is_an_error() {
        let mut bytes = build_file(&[(1, "Delphine")], false);
        bytes.truncate(bytes.len() - 3);
        assert!(matches!(
            StringsFile::from_bytes(&bytes, StringsKind::General),
            Err(StringsError::Truncated(_))
        ));
    }

    #[test]
    fn empty_file_parses() {
        let bytes = build_file(&[], false);
        let file = StringsFile::from_bytes(&bytes, StringsKind::General).unwrap();
        assert!(file.is_empty());
        assert_eq!(file.get(1), None);
    }

    #[test]
    fn out_of_range_offset_degrades_to_empty() {
        // A directory entry pointing past the data section
        // yields an empty string, not a panic.
        let mut bytes = build_file(&[(1, "Delphine")], false);
        // Rewrite entry 0's offset to a huge value.
        bytes[12..16].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        let file = StringsFile::from_bytes(&bytes, StringsKind::General).unwrap();
        assert_eq!(file.get(1), Some(""));
    }
}
