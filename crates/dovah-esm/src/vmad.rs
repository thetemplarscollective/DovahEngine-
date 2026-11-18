//! VMAD field parser — the Papyrus script data attached to a record.
//!
//! Any record that has scripts carries a `VMAD` field: the names of
//! the attached scripts, the initial values of their properties (as
//! set in the Creation Kit), and — for INFO, PACK, PERK, QUST, and
//! SCEN records — script fragments. Format per the UESP wiki
//! ("Skyrim Mod:Mod File Format/VMAD Field"); see
//! `docs/formats/vmad.md` for the full layout.
//!
//! All numbers are little-endian (unlike PEX, which is big-endian).
//! Strings are `wstring`s: a u16 length followed by that many bytes,
//! Windows-1252 encoded, no terminator.

use crate::{Code, EsmError, Result};

/// A parsed VMAD field: the scripts attached to one record.
#[derive(Debug, Clone, PartialEq)]
pub struct Vmad {
    /// VMAD version: 5 is the most recent, 2 the minimum. Decides
    /// whether status bytes are present (>= 4) and whether array
    /// property types are allowed (>= 5).
    pub version: i16,
    /// Object format: 1 or 2. Decides the byte order of object-type
    /// property values.
    pub obj_format: i16,
    /// Scripts directly attached to the record.
    pub scripts: Vec<VmadScript>,
    /// Script fragments, present only on INFO, PACK, PERK, QUST,
    /// and SCEN records that have them.
    pub fragments: Option<Fragments>,
}

/// One script attached to a record, with its initial property
/// values.
#[derive(Debug, Clone, PartialEq)]
pub struct VmadScript {
    /// Script name, without the `.pex` extension.
    pub name: String,
    /// Script status (only stored when version >= 4; 0 otherwise):
    /// 0 = local script, 1 = inherited with altered properties,
    /// 3 = inherited and then removed.
    pub status: u8,
    pub properties: Vec<VmadProperty>,
}

/// One property's initial value, as set in the Creation Kit.
#[derive(Debug, Clone, PartialEq)]
pub struct VmadProperty {
    pub name: String,
    /// Property status (only stored when version >= 4; 1 otherwise):
    /// 1 = property edited, 3 = property removed.
    pub status: u8,
    pub value: VmadValue,
}

/// A property value. Object values carry a form ID and, when the
/// form is a quest, the alias ID the property points at.
#[derive(Debug, Clone, PartialEq)]
pub enum VmadValue {
    /// An object reference. `alias_id` is `Some` only when the
    /// property points at a quest alias; `None` (alias 0xFFFF in
    /// the file) means the form ID is assigned directly.
    Object {
        form_id: u32,
        alias_id: Option<u16>,
    },
    Str(String),
    Int(i32),
    Float(f32),
    Bool(bool),
    /// An array property (types 11–15). Elements all share one
    /// type; the element values are stored as their own
    /// `VmadValue`s (never nested arrays).
    Array(Vec<VmadValue>),
}

/// One script fragment: a function inside a fragment script file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    /// Name of the script file containing the fragment (normally
    /// the same as the fragment section's file name).
    pub script_name: String,
    /// Name of the function holding the fragment, e.g.
    /// `"Fragment_5"`.
    pub fragment_name: String,
}

/// A PERK fragment: indexed, so the index is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFragment {
    pub index: u16,
    pub script_name: String,
    pub fragment_name: String,
}

/// A QUST fragment: attached to one quest stage (and optionally one
/// log entry within it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestFragment {
    /// Quest stage index (the QUST `INDX` value).
    pub stage_index: u16,
    /// Log entry within the stage, or -1 when the fragment is
    /// attached to the stage as a whole.
    pub log_entry: i32,
    pub script_name: String,
    pub fragment_name: String,
}

/// Scripts attached to one quest alias (QUST fragments section).
#[derive(Debug, Clone, PartialEq)]
pub struct QuestAlias {
    /// The quest the alias belongs to (the form ID of the quest
    /// holding this VMAD field, in practice).
    pub form_id: u32,
    pub alias_id: Option<u16>,
    pub scripts: Vec<VmadScript>,
}

/// A SCEN phase fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseFragment {
    /// Phase number, 0-based in the data (1-based in the CK).
    pub phase: u32,
    pub script_name: String,
    pub fragment_name: String,
}

/// The fragments section of a VMAD field. Its layout depends on the
/// record type; only INFO, PACK, PERK, QUST, and SCEN records ever
/// have one, and it is absent (the field just ends) when a record
/// has no fragments.
#[derive(Debug, Clone, PartialEq)]
pub enum Fragments {
    /// INFO / PACK: begin/end (and, for PACK, change) fragments,
    /// selected by the bits in `flags`.
    Flagged {
        /// Fragment script file name, e.g. `TIF_<edid>_<formid>`.
        file_name: String,
        /// 0x1 = has begin script, 0x2 = has end script,
        /// 0x4 = has change script (PACK only).
        flags: u8,
        fragments: Vec<Fragment>,
    },
    /// PERK: indexed fragments.
    Perk {
        file_name: String,
        fragments: Vec<IndexedFragment>,
    },
    /// QUST: stage fragments plus scripts attached to quest aliases.
    Quest {
        file_name: String,
        fragments: Vec<QuestFragment>,
        aliases: Vec<QuestAlias>,
    },
    /// SCEN: begin/end fragments plus phase fragments.
    Scene {
        file_name: String,
        /// 0x1 = has begin script, 0x2 = has end script.
        flags: u8,
        begin_end: Vec<Fragment>,
        phases: Vec<PhaseFragment>,
    },
}

impl Vmad {
    /// Parse a VMAD field's payload. `record_type` decides how a
    /// trailing fragments section (if any) is read; only INFO,
    /// PACK, PERK, QUST, and SCEN may carry one.
    pub fn parse(data: &[u8], record_type: &Code) -> Result<Vmad> {
        let mut reader = Reader::new(data);
        let version = reader.i16("VMAD version")?;
        if !(2..=5).contains(&version) {
            return Err(EsmError::InvalidData("VMAD version"));
        }
        let obj_format = reader.i16("VMAD object format")?;
        if obj_format != 1 && obj_format != 2 {
            return Err(EsmError::InvalidData("VMAD object format"));
        }
        let script_count = reader.u16("VMAD script count")? as usize;
        let mut scripts = Vec::with_capacity(script_count);
        for _ in 0..script_count {
            scripts.push(parse_script(&mut reader, version, obj_format)?);
        }
        let fragments = if reader.is_empty() {
            None
        } else {
            Some(parse_fragments(
                &mut reader,
                record_type,
                version,
                obj_format,
            )?)
        };
        if !reader.is_empty() {
            return Err(EsmError::InvalidData("trailing bytes in VMAD field"));
        }
        Ok(Vmad {
            version,
            obj_format,
            scripts,
            fragments,
        })
    }

    /// Find an attached script by name.
    pub fn script(&self, name: &str) -> Option<&VmadScript> {
        self.scripts.iter().find(|s| s.name == name)
    }
}

/// A bounds-checked little-endian cursor over the VMAD bytes.
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

    fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.take(1, what)?[0])
    }

    fn i8(&mut self, what: &'static str) -> Result<i8> {
        Ok(self.take(1, what)?[0] as i8)
    }

    fn u16(&mut self, what: &'static str) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2, what)?.try_into().unwrap()))
    }

    fn i16(&mut self, what: &'static str) -> Result<i16> {
        Ok(i16::from_le_bytes(self.take(2, what)?.try_into().unwrap()))
    }

    fn u32(&mut self, what: &'static str) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    fn i32(&mut self, what: &'static str) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    fn f32(&mut self, what: &'static str) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    /// A VMAD `wstring`: u16 little-endian byte length, then that
    /// many Windows-1252 bytes, no terminator.
    fn wstring(&mut self, what: &'static str) -> Result<String> {
        let len = self.u16(what)? as usize;
        let bytes = self.take(len, what)?;
        Ok(decode_windows1252(bytes))
    }

    /// An 8-byte object reference: a form ID plus a quest alias ID.
    /// The byte order depends on `obj_format` (see UESP): format 1
    /// is form ID, alias, unused; format 2 is unused, alias, form
    /// ID. Alias 0xFFFF means "no alias".
    fn object_ref(&mut self, obj_format: i16, what: &'static str) -> Result<(u32, Option<u16>)> {
        let bytes = self.take(8, what)?;
        let (form_id, alias_raw) = if obj_format == 1 {
            (
                u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
                u16::from_le_bytes(bytes[4..6].try_into().unwrap()),
            )
        } else {
            (
                u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
                u16::from_le_bytes(bytes[2..4].try_into().unwrap()),
            )
        };
        let alias_id = if alias_raw == 0xFFFF {
            None
        } else {
            Some(alias_raw)
        };
        Ok((form_id, alias_id))
    }
}

/// Decode Windows-1252 bytes into a `String` (Bethesda's string
/// encoding). Bytes 0x00–0x7F and 0xA0–0xFF map straight to the same
/// Unicode code points; 0x80–0x9F use the Windows-1252 table, with
/// the five undefined positions becoming U+FFFD.
fn decode_windows1252(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| windows1252_char(b)).collect()
}

fn windows1252_char(b: u8) -> char {
    match b {
        0x80 => '\u{20AC}',
        0x82 => '\u{201A}',
        0x83 => '\u{0192}',
        0x84 => '\u{201E}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02C6}',
        0x89 => '\u{2030}',
        0x8A => '\u{0160}',
        0x8B => '\u{2039}',
        0x8C => '\u{0152}',
        0x8E => '\u{017D}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x9A => '\u{0161}',
        0x9B => '\u{203A}',
        0x9C => '\u{0153}',
        0x9E => '\u{017E}',
        0x9F => '\u{0178}',
        0x81 | 0x8D | 0x8F | 0x90 | 0x9D => '\u{FFFD}',
        _ => b as char,
    }
}

fn parse_script(reader: &mut Reader, version: i16, obj_format: i16) -> Result<VmadScript> {
    let name = reader.wstring("VMAD script name")?;
    let status = if version >= 4 {
        reader.u8("VMAD script status")?
    } else {
        0
    };
    let property_count = reader.u16("VMAD property count")? as usize;
    let mut properties = Vec::with_capacity(property_count);
    for _ in 0..property_count {
        properties.push(parse_property(reader, version, obj_format)?);
    }
    Ok(VmadScript {
        name,
        status,
        properties,
    })
}

fn parse_property(reader: &mut Reader, version: i16, obj_format: i16) -> Result<VmadProperty> {
    let name = reader.wstring("VMAD property name")?;
    let property_type = reader.u8("VMAD property type")?;
    let status = if version >= 4 {
        reader.u8("VMAD property status")?
    } else {
        1
    };
    let value = parse_value(reader, property_type, version, obj_format)?;
    Ok(VmadProperty {
        name,
        status,
        value,
    })
}

/// Parse one property value of the given type code. Array types
/// (11–15) are a u32 count followed by that many elements of the
/// matching scalar type.
fn parse_value(
    reader: &mut Reader,
    property_type: u8,
    version: i16,
    obj_format: i16,
) -> Result<VmadValue> {
    match property_type {
        1 => {
            let (form_id, alias_id) = reader.object_ref(obj_format, "VMAD object property")?;
            Ok(VmadValue::Object { form_id, alias_id })
        }
        2 => Ok(VmadValue::Str(reader.wstring("VMAD string property")?)),
        3 => Ok(VmadValue::Int(reader.i32("VMAD int property")?)),
        4 => Ok(VmadValue::Float(reader.f32("VMAD float property")?)),
        5 => Ok(VmadValue::Bool(reader.u8("VMAD bool property")? != 0)),
        11..=15 => {
            if version < 5 {
                return Err(EsmError::InvalidData(
                    "VMAD array property before version 5",
                ));
            }
            // Array element types mirror the scalar codes minus 10.
            let element_type = property_type - 10;
            let count = reader.u32("VMAD array count")? as usize;
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                items.push(parse_value(reader, element_type, version, obj_format)?);
            }
            Ok(VmadValue::Array(items))
        }
        _ => Err(EsmError::InvalidData("VMAD property type")),
    }
}

fn parse_fragments(
    reader: &mut Reader,
    record_type: &Code,
    version: i16,
    obj_format: i16,
) -> Result<Fragments> {
    match record_type {
        b"INFO" | b"PACK" => {
            let unknown = reader.i8("VMAD fragment unknown")?;
            if unknown != 2 {
                return Err(EsmError::InvalidData("VMAD fragment marker"));
            }
            let flags = reader.u8("VMAD fragment flags")?;
            let file_name = reader.wstring("VMAD fragment file name")?;
            let count = flags.count_ones() as usize;
            let mut fragments = Vec::with_capacity(count);
            for _ in 0..count {
                fragments.push(parse_fragment(reader)?);
            }
            Ok(Fragments::Flagged {
                file_name,
                flags,
                fragments,
            })
        }
        b"PERK" => {
            let unknown = reader.i8("VMAD fragment unknown")?;
            if unknown != 2 {
                return Err(EsmError::InvalidData("VMAD fragment marker"));
            }
            let file_name = reader.wstring("VMAD fragment file name")?;
            let count = reader.u16("VMAD fragment count")? as usize;
            let mut fragments = Vec::with_capacity(count);
            for _ in 0..count {
                let index = reader.u16("VMAD fragment index")?;
                let _unknown2 = reader.i16("VMAD fragment unknown")?;
                let _unknown3 = reader.u8("VMAD fragment unknown")?;
                let script_name = reader.wstring("VMAD fragment script name")?;
                let fragment_name = reader.wstring("VMAD fragment name")?;
                fragments.push(IndexedFragment {
                    index,
                    script_name,
                    fragment_name,
                });
            }
            Ok(Fragments::Perk {
                file_name,
                fragments,
            })
        }
        b"QUST" => {
            let unknown = reader.i8("VMAD fragment unknown")?;
            if unknown != 2 {
                return Err(EsmError::InvalidData("VMAD fragment marker"));
            }
            let count = reader.u16("VMAD fragment count")? as usize;
            let file_name = reader.wstring("VMAD fragment file name")?;
            let mut fragments = Vec::with_capacity(count);
            for _ in 0..count {
                let stage_index = reader.u16("VMAD fragment stage")?;
                let _unknown2 = reader.i16("VMAD fragment unknown")?;
                let log_entry = reader.i32("VMAD fragment log entry")?;
                let _unknown3 = reader.u8("VMAD fragment unknown")?;
                let script_name = reader.wstring("VMAD fragment script name")?;
                let fragment_name = reader.wstring("VMAD fragment name")?;
                fragments.push(QuestFragment {
                    stage_index,
                    log_entry,
                    script_name,
                    fragment_name,
                });
            }
            let alias_count = reader.u16("VMAD alias count")? as usize;
            let mut aliases = Vec::with_capacity(alias_count);
            for _ in 0..alias_count {
                let (form_id, alias_id) = reader.object_ref(obj_format, "VMAD alias object")?;
                let alias_version = reader.i16("VMAD alias version")?;
                let alias_obj_format = reader.i16("VMAD alias object format")?;
                if alias_version != version || alias_obj_format != obj_format {
                    return Err(EsmError::InvalidData("VMAD alias version mismatch"));
                }
                let alias_script_count = reader.u16("VMAD alias script count")? as usize;
                let mut scripts = Vec::with_capacity(alias_script_count);
                for _ in 0..alias_script_count {
                    scripts.push(parse_script(reader, version, obj_format)?);
                }
                aliases.push(QuestAlias {
                    form_id,
                    alias_id,
                    scripts,
                });
            }
            Ok(Fragments::Quest {
                file_name,
                fragments,
                aliases,
            })
        }
        b"SCEN" => {
            let unknown = reader.i8("VMAD fragment unknown")?;
            if unknown != 2 {
                return Err(EsmError::InvalidData("VMAD fragment marker"));
            }
            let flags = reader.u8("VMAD fragment flags")?;
            let file_name = reader.wstring("VMAD fragment file name")?;
            let be_count = flags.count_ones() as usize;
            let mut begin_end = Vec::with_capacity(be_count);
            for _ in 0..be_count {
                begin_end.push(parse_fragment(reader)?);
            }
            let phase_count = reader.u16("VMAD phase count")? as usize;
            let mut phases = Vec::with_capacity(phase_count);
            for _ in 0..phase_count {
                let _unknown2 = reader.u8("VMAD phase unknown")?;
                let phase = reader.u32("VMAD phase number")?;
                let _unknown3 = reader.u8("VMAD phase unknown")?;
                let script_name = reader.wstring("VMAD phase script name")?;
                let fragment_name = reader.wstring("VMAD phase fragment name")?;
                phases.push(PhaseFragment {
                    phase,
                    script_name,
                    fragment_name,
                });
            }
            Ok(Fragments::Scene {
                file_name,
                flags,
                begin_end,
                phases,
            })
        }
        _ => Err(EsmError::InvalidData(
            "VMAD fragments on a record type that never has them",
        )),
    }
}

/// One begin/end/change fragment: an unknown byte, then the script
/// and fragment names.
fn parse_fragment(reader: &mut Reader) -> Result<Fragment> {
    let _unknown = reader.u8("VMAD fragment unknown")?;
    let script_name = reader.wstring("VMAD fragment script name")?;
    let fragment_name = reader.wstring("VMAD fragment name")?;
    Ok(Fragment {
        script_name,
        fragment_name,
    })
}
