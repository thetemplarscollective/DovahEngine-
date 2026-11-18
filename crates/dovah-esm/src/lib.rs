//! ESM/ESP plugin parser for Dovahengine.
//!
//! Reads Bethesda plugin files (Skyrim Special Edition), following the
//! public UESP format documentation (`docs/formats/esm.md`). This crate
//! contains no Bethesda code and ships no game data: it parses plugin
//! files the user already owns.
//!
//! The parser reads a whole plugin into memory. That is fine for
//! Skyrim.esm (~250 MB) on the build machine, but a lazy, seek-based
//! index is future work for the renderer phase — see the notes file.
//!
//! Field payloads stay raw bytes, except for three interpreted
//! fields: `EDID` as a string, the TES4 `HEDR` struct (see
//! [`Plugin::header_info`]), and `VMAD` script data (see the
//! [`vmad`] module and [`Record::vmad`]). Other per-record field
//! semantics are a later milestone.

pub mod dialogue;
pub mod map;
pub mod quest;
pub mod skills;
pub mod spell;
pub mod vmad;
pub mod water;

pub use dialogue::{ctda_get_is_id, full_string_id, parse_dial, parse_info, topic_children};
pub use quest::{Quest, QuestObjective, QuestStageText, parse_quest};
pub use vmad::{
    Fragment, Fragments, IndexedFragment, PhaseFragment, QuestAlias, QuestFragment, Vmad,
    VmadProperty, VmadScript, VmadValue,
};

use std::fs;
use std::io::{self, Read};
use std::path::Path;

/// Record flags (see UESP "Records").
pub const FLAG_MASTER: u32 = 0x0000_0001;
pub const FLAG_DELETED: u32 = 0x0000_0020;
pub const FLAG_LOCALIZED: u32 = 0x0000_0080;
pub const FLAG_LIGHT_MASTER: u32 = 0x0000_0200;
pub const FLAG_COMPRESSED: u32 = 0x0004_0000;

const RECORD_HEADER_LEN: usize = 24;
const GROUP_HEADER_LEN: usize = 24;
const FIELD_HEADER_LEN: usize = 6;

/// A four-character code, as stored in the file (e.g. `TES4`, `GMST`,
/// `EDID`). Kept as raw bytes; [`code_str`] renders it lossily.
pub type Code = [u8; 4];

/// Render a four-character code as a string, replacing any non-ASCII
/// byte with the replacement character.
pub fn code_str(code: &Code) -> String {
    String::from_utf8_lossy(code).into_owned()
}

#[derive(Debug)]
pub enum EsmError {
    Io(io::Error),
    /// The first record in the file is not TES4.
    NotAPlugin,
    Truncated(&'static str),
    InvalidData(&'static str),
    Decompress,
}

impl std::fmt::Display for EsmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EsmError::Io(e) => write!(f, "I/O error: {e}"),
            EsmError::NotAPlugin => {
                write!(f, "not a plugin file (first record is not TES4)")
            }
            EsmError::Truncated(what) => write!(f, "truncated plugin in {what}"),
            EsmError::InvalidData(what) => write!(f, "invalid plugin data: {what}"),
            EsmError::Decompress => {
                write!(f, "failed to decompress a compressed record (zlib)")
            }
        }
    }
}

impl std::error::Error for EsmError {}

impl From<io::Error> for EsmError {
    fn from(e: io::Error) -> Self {
        EsmError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, EsmError>;

/// A single field (subrecord) of a record: a four-character type and
/// its raw payload bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub field_type: Code,
    pub data: Vec<u8>,
}

impl Field {
    /// The field type as a string, e.g. `"EDID"`.
    pub fn type_str(&self) -> String {
        code_str(&self.field_type)
    }

    /// Interpret the payload as a NUL-terminated string (zstring).
    /// Trailing NUL bytes are stripped; invalid UTF-8 is replaced.
    pub fn as_str(&self) -> String {
        let end = self
            .data
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(self.data.len());
        String::from_utf8_lossy(&self.data[..end]).into_owned()
    }
}

/// One record: a typed object (weapon, NPC, cell, ...) with a form ID
/// and a list of fields.
#[derive(Debug, Clone)]
pub struct Record {
    pub record_type: Code,
    pub flags: u32,
    pub form_id: u32,
    /// Internal record version (Skyrim LE = 43, Skyrim SE = 44).
    pub version: u16,
    pub fields: Vec<Field>,
}

impl Record {
    /// The record type as a string, e.g. `"WEAP"`.
    pub fn type_str(&self) -> String {
        code_str(&self.record_type)
    }

    /// The first field of the given type, if any.
    pub fn field(&self, field_type: &Code) -> Option<&Field> {
        self.fields.iter().find(|f| &f.field_type == field_type)
    }

    /// All fields of the given type, in file order.
    pub fn fields<'a>(&'a self, field_type: &'a Code) -> impl Iterator<Item = &'a Field> + 'a {
        self.fields
            .iter()
            .filter(move |f| &f.field_type == field_type)
    }

    /// The record's editor ID (`EDID` field), if it has one.
    pub fn edid(&self) -> Option<String> {
        self.field(b"EDID").map(|f| f.as_str())
    }

    /// The record's script data (`VMAD` field), parsed. `None` when
    /// the record has no VMAD field; `Some(Err(..))` when the field
    /// is present but malformed.
    pub fn vmad(&self) -> Option<Result<Vmad>> {
        self.field(b"VMAD")
            .map(|f| Vmad::parse(&f.data, &self.record_type))
    }

    /// Whether the record's data was stored compressed (flag
    /// `0x00040000`). Fields are already decompressed by the parser.
    pub fn was_compressed(&self) -> bool {
        self.flags & FLAG_COMPRESSED != 0
    }
}

/// A group type, with the meaning of its label (see UESP "Groups").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupType {
    /// Top group; label is the record type, e.g. `GMST`.
    Top,
    /// Children of a worldspace; label is the parent WRLD form ID.
    WorldChildren,
    /// Interior cell block; label is the block number (i32).
    InteriorCellBlock,
    /// Interior cell sub-block; label is the sub-block number (i32).
    InteriorCellSubBlock,
    /// Exterior cell block; label is grid Y, X (i16[2], reverse order).
    ExteriorCellBlock,
    /// Exterior cell sub-block; label is grid Y, X (i16[2]).
    ExteriorCellSubBlock,
    /// Children of a cell; label is the parent CELL form ID.
    CellChildren,
    /// Children of a dialog topic; label is the parent DIAL form ID.
    TopicChildren,
    /// Persistent children of a cell; label is the parent CELL form ID.
    CellPersistentChildren,
    /// Temporary children of a cell; label is the parent CELL form ID.
    CellTemporaryChildren,
    /// Any other type value, kept verbatim.
    Unknown(i32),
}

impl GroupType {
    pub fn from_i32(value: i32) -> Self {
        match value {
            0 => GroupType::Top,
            1 => GroupType::WorldChildren,
            2 => GroupType::InteriorCellBlock,
            3 => GroupType::InteriorCellSubBlock,
            4 => GroupType::ExteriorCellBlock,
            5 => GroupType::ExteriorCellSubBlock,
            6 => GroupType::CellChildren,
            7 => GroupType::TopicChildren,
            8 => GroupType::CellPersistentChildren,
            9 => GroupType::CellTemporaryChildren,
            other => GroupType::Unknown(other),
        }
    }

    pub fn to_i32(self) -> i32 {
        match self {
            GroupType::Top => 0,
            GroupType::WorldChildren => 1,
            GroupType::InteriorCellBlock => 2,
            GroupType::InteriorCellSubBlock => 3,
            GroupType::ExteriorCellBlock => 4,
            GroupType::ExteriorCellSubBlock => 5,
            GroupType::CellChildren => 6,
            GroupType::TopicChildren => 7,
            GroupType::CellPersistentChildren => 8,
            GroupType::CellTemporaryChildren => 9,
            GroupType::Unknown(value) => value,
        }
    }
}

/// One child of a group: either a record or a nested subgroup.
#[derive(Debug, Clone)]
pub enum GroupChild {
    Record(Record),
    Group(Group),
}

/// A GRUP: a collection of records (and/or subgroups) sharing one
/// parent or record type.
#[derive(Debug, Clone)]
pub struct Group {
    /// Raw label bytes. Interpret with [`Group::label_type`] or
    /// [`Group::label_form_id`] depending on the group type.
    pub label: [u8; 4],
    pub group_type: GroupType,
    pub children: Vec<GroupChild>,
}

impl Group {
    /// The label as a record type, for top groups (type 0).
    pub fn label_type(&self) -> Option<String> {
        if self.group_type == GroupType::Top {
            Some(code_str(&self.label))
        } else {
            None
        }
    }

    /// The label as a parent form ID, for child groups (types 1, 6, 7,
    /// 8, 9).
    pub fn label_form_id(&self) -> Option<u32> {
        match self.group_type {
            GroupType::WorldChildren
            | GroupType::CellChildren
            | GroupType::TopicChildren
            | GroupType::CellPersistentChildren
            | GroupType::CellTemporaryChildren => Some(u32::from_le_bytes(self.label)),
            _ => None,
        }
    }

    /// All records directly in this group (not in nested subgroups).
    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.children.iter().filter_map(|c| match c {
            GroupChild::Record(record) => Some(record),
            GroupChild::Group(_) => None,
        })
    }

    /// All subgroups directly in this group.
    pub fn subgroups(&self) -> impl Iterator<Item = &Group> {
        self.children.iter().filter_map(|c| match c {
            GroupChild::Group(group) => Some(group),
            GroupChild::Record(_) => None,
        })
    }
}

/// The parsed TES4 `HEDR` struct: file version, record/group count,
/// and next available object ID.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeaderInfo {
    pub version: f32,
    pub record_count: u32,
    pub next_object_id: u32,
}

/// A parsed plugin file: one TES4 header record and its top groups.
#[derive(Debug)]
pub struct Plugin {
    pub header: Record,
    pub groups: Vec<Group>,
}

impl Plugin {
    /// Parse a plugin file from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    /// Parse a plugin from its bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);
        let header = parse_record(&mut reader)?;
        if &header.record_type != b"TES4" {
            return Err(EsmError::NotAPlugin);
        }
        let mut groups = Vec::new();
        while !reader.is_empty() {
            groups.push(parse_group(&mut reader)?);
        }
        Ok(Plugin { header, groups })
    }

    /// The TES4 `HEDR` header info, if the header has a well-formed
    /// 12-byte HEDR field.
    pub fn header_info(&self) -> Option<HeaderInfo> {
        let hedr = self.header.field(b"HEDR")?;
        if hedr.data.len() != 12 {
            return None;
        }
        Some(HeaderInfo {
            version: f32::from_le_bytes(hedr.data[0..4].try_into().unwrap()),
            record_count: u32::from_le_bytes(hedr.data[4..8].try_into().unwrap()),
            next_object_id: u32::from_le_bytes(hedr.data[8..12].try_into().unwrap()),
        })
    }

    /// Master filenames from the TES4 `MAST` fields, in load order.
    pub fn masters(&self) -> Vec<String> {
        self.header.fields(b"MAST").map(|f| f.as_str()).collect()
    }

    /// All records in the plugin, in file order, across all groups
    /// and subgroups (not including the TES4 header itself).
    pub fn all_records(&self) -> Vec<&Record> {
        let mut out = Vec::new();
        for group in &self.groups {
            collect_records(group, &mut out);
        }
        out
    }

    /// Find a record by form ID anywhere in the plugin.
    pub fn find(&self, form_id: u32) -> Option<&Record> {
        self.all_records()
            .into_iter()
            .find(|r| r.form_id == form_id)
    }
}

fn collect_records<'a>(group: &'a Group, out: &mut Vec<&'a Record>) {
    for child in &group.children {
        match child {
            GroupChild::Record(record) => out.push(record),
            GroupChild::Group(subgroup) => collect_records(subgroup, out),
        }
    }
}

/// A bounds-checked cursor over the plugin bytes.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, pos: 0 }
    }

    fn is_empty(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn take(&mut self, len: usize, what: &'static str) -> Result<&'a [u8]> {
        if self.remaining() < len {
            return Err(EsmError::Truncated(what));
        }
        let slice = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn code(&mut self, what: &'static str) -> Result<Code> {
        Ok(self.take(4, what)?.try_into().unwrap())
    }

    fn u16(&mut self, what: &'static str) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2, what)?.try_into().unwrap()))
    }

    fn u32(&mut self, what: &'static str) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    fn i32(&mut self, what: &'static str) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }
}

fn parse_record(reader: &mut Reader) -> Result<Record> {
    if reader.remaining() < RECORD_HEADER_LEN {
        return Err(EsmError::Truncated("record header"));
    }
    let record_type = reader.code("record type")?;
    let data_size = reader.u32("record size")? as usize;
    let flags = reader.u32("record flags")?;
    let form_id = reader.u32("record form ID")?;
    let _timestamp = reader.u16("record timestamp")?;
    let _version_control = reader.u16("record version control")?;
    let version = reader.u16("record version")?;
    let _unknown = reader.u16("record unknown")?;
    let data = reader.take(data_size, "record data")?;

    let fields = if flags & FLAG_COMPRESSED != 0 {
        parse_compressed_fields(data)?
    } else {
        parse_fields(data)?
    };
    Ok(Record {
        record_type,
        flags,
        form_id,
        version,
        fields,
    })
}

/// Decompress a compressed record's data (u32 decompressed size +
/// zlib payload) and parse the fields from the result.
fn parse_compressed_fields(data: &[u8]) -> Result<Vec<Field>> {
    if data.len() < 4 {
        return Err(EsmError::Truncated("compressed size prefix"));
    }
    let decompressed_size = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let mut decoder = flate2::read::ZlibDecoder::new(&data[4..]);
    let mut out = Vec::with_capacity(decompressed_size);
    decoder
        .read_to_end(&mut out)
        .map_err(|_| EsmError::Decompress)?;
    parse_fields(&out)
}

/// Parse a run of fields from a record's (decompressed) data bytes.
///
/// Handles the XXXX escape: a field of type `XXXX` carries the real
/// u32 size of the *following* field, whose own u16 size is 0. XXXX
/// itself is not kept as a field.
fn parse_fields(data: &[u8]) -> Result<Vec<Field>> {
    let mut reader = Reader::new(data);
    let mut fields = Vec::new();
    let mut oversized: Option<u32> = None;
    while !reader.is_empty() {
        if reader.remaining() < FIELD_HEADER_LEN {
            return Err(EsmError::Truncated("field header"));
        }
        let field_type = reader.code("field type")?;
        let size_u16 = reader.u16("field size")? as usize;
        if &field_type == b"XXXX" {
            if size_u16 != 4 {
                return Err(EsmError::InvalidData("XXXX field size"));
            }
            oversized = Some(reader.u32("XXXX size")?);
            continue;
        }
        let size = match oversized.take() {
            Some(size) => size as usize,
            None => size_u16,
        };
        let payload = reader.take(size, "field data")?;
        fields.push(Field {
            field_type,
            data: payload.to_vec(),
        });
    }
    if oversized.is_some() {
        return Err(EsmError::InvalidData("dangling XXXX field"));
    }
    Ok(fields)
}

fn parse_group(reader: &mut Reader) -> Result<Group> {
    if reader.remaining() < GROUP_HEADER_LEN {
        return Err(EsmError::Truncated("group header"));
    }
    let group_type_code = reader.code("group type")?;
    if &group_type_code != b"GRUP" {
        return Err(EsmError::InvalidData("expected GRUP"));
    }
    // Group size includes the 24-byte header.
    let size = reader.u32("group size")? as usize;
    if size < GROUP_HEADER_LEN {
        return Err(EsmError::InvalidData("group size"));
    }
    let label: [u8; 4] = reader.take(4, "group label")?.try_into().unwrap();
    let group_type = GroupType::from_i32(reader.i32("group type value")?);
    let _timestamp = reader.u16("group timestamp")?;
    let _version_control = reader.u16("group version control")?;
    let _unknown = reader.u32("group unknown")?;

    let body_len = size - GROUP_HEADER_LEN;
    let body = reader.take(body_len, "group body")?;
    let mut body_reader = Reader::new(body);
    let mut children = Vec::new();
    while !body_reader.is_empty() {
        if body_reader.remaining() < 4 {
            return Err(EsmError::Truncated("group child"));
        }
        let peek: Code = body_reader.bytes[body_reader.pos..body_reader.pos + 4]
            .try_into()
            .unwrap();
        if &peek == b"GRUP" {
            children.push(GroupChild::Group(parse_group(&mut body_reader)?));
        } else {
            children.push(GroupChild::Record(parse_record(&mut body_reader)?));
        }
    }
    Ok(Group {
        label,
        group_type,
        children,
    })
}
