//! DIAL (dialogue topic) and INFO (response) parsing for M18.
//!
//! Every fact here is proven from Dylan's real `Skyrim.esm` by
//! the `dialogue_probe` / `dialogue_probe2` / `dialogue_probe3` /
//! `dialogue_probe4` examples in dovah-app (kept as the durable
//! research record). Counts below are the real file's counts:
//!
//! * DIAL records: **15,037**. 6,945 with EDID; 5,170 with FULL.
//!   Most DIALs have NO EDID and NO FULL — scene/forced topics,
//!   not player-selectable.
//! * INFO records: **31,465**, under 14,904 topics
//!   (`TopicChildren` groups, group type 7, label = parent DIAL
//!   form ID). 133 DIALs have no INFO children.
//!
//! ## DIAL fields
//!
//! * `EDID` (sometimes) — editor ID.
//! * `FULL` (4 bytes when present) — a string ID into
//!   `.STRINGS`: the topic text, i.e. the PLAYER's line.
//! * `SNAM` (4-char code) — the subtype. `CUST` = custom,
//!   player-selectable topics (6,503); `HELO` = greeting (297);
//!   `GBYE` = goodbye (129); `SCEN` = scene dialogue (7,426);
//!   plus combat barks (`HIT_`, `DETH`, `ATCK`), ambient
//!   (`IDLE`, `NOTC`), and dozens of singletons. Every DIAL
//!   appears to have an SNAM. **Use SNAM as the classifier** —
//!   it is exact and readable.
//! * `DATA` (4 bytes) correlates exactly with SNAM: byte1 is
//!   0 for player topics (CUST/RUMO), 2 for scene, 3 for
//!   combat barks, 7 for ambient (HELO/GBYE/IDLE); byte2 is a
//!   sub-index; byte0 is a flag (bit0 set on a few). M18 uses
//!   SNAM, not DATA.
//! * `PNAM` (4 bytes, f32) — priority (50.0/80.0/40.0 seen).
//! * `QNAM` (4 bytes) — quest form ID. `BNAM` — branch topic
//!   form ID (sometimes). `TIFC` (4 bytes, =1 seen) — likely
//!   the INFO count.
//!
//! Greetings are identified by `SNAM == "HELO"`. EDID matching
//! does NOT work for greetings (only 9 DIALs have "Greeting" in
//! the EDID, all quest branch CUST topics).
//!
//! ## INFO fields
//!
//! * `NAM1` (4 bytes) — a string ID into **`.ILSTRINGS`**: the
//!   response text, i.e. what the NPC says back. (NOT
//!   `.DLSTRINGS` — the remembered docs are wrong for Skyrim SE;
//!   see dovah-strings.) One INFO can hold MULTIPLE
//!   (`TRDT`, `NAM1`, `NAM2`, `NAM3`) response groups —
//!   alternative lines; the game picks one. M18 takes the
//!   first resolvable NAM1 (documented approximation).
//! * Some INFOs are script-only and have NO NAM1 (e.g. INFO
//!   `0x00016f8c`: VMAD + DATA + CNAM + DNAM + CTDA + SCHR +
//!   QNAM, no spoken line). Callers must skip those.
//! * `TRDT` (24 bytes, six u32s) — the response header. Field
//!   index 3 is the response index within the INFO (1, 2, 3
//!   seen); fields 0/1 may be emotion type/value — unverified,
//!   not needed for M18.
//! * `CTDA` (32 bytes) — one condition. See
//!   [`ctda_get_is_id`] for the layout and the u16 trap.
//!
//! INFO file order is the game's priority order (assumed; no
//! explicit priority field was found). Callers take the first
//! targeting INFO in file order.
//!
//! ## What M18 does NOT do
//!
//! Full condition evaluation (quest stages, factions, items,
//! etc.) is out of reach this milestone: M18 shows all topics
//! for the NPC (targeted by GetIsID) and says so plainly. No
//! gating is faked.

use std::collections::HashMap;

use crate::{Group, GroupType, Plugin, Record};

/// Condition function 72 (`0x48`) = `GetIsID`: the condition
/// passes when the speaker is the NPC in param1.
pub const CTDA_FUNC_GET_IS_ID: u16 = 72;

/// A parsed DIAL record (dialogue topic).
#[derive(Debug, Clone, PartialEq)]
pub struct Dial {
    pub form_id: u32,
    pub edid: Option<String>,
    /// Topic text string ID (`FULL` field) — resolves in
    /// `.STRINGS`. `None` when the DIAL has no FULL field (most
    /// scene/forced topics).
    pub full_id: Option<u32>,
    /// Subtype code (`SNAM`), e.g. `"CUST"`, `"HELO"`, `"SCEN"`.
    pub snam: Option<String>,
    /// Quest form ID (`QNAM`).
    pub quest: Option<u32>,
    /// Priority (`PNAM`, f32).
    pub priority: Option<f32>,
}

impl Dial {
    /// Whether this is a greeting topic (`SNAM == "HELO"`).
    pub fn is_greeting(&self) -> bool {
        self.snam.as_deref() == Some("HELO")
    }

    /// Whether this is a player-selectable topic (`SNAM ==
    /// "CUST"`).
    pub fn is_player_topic(&self) -> bool {
        self.snam.as_deref() == Some("CUST")
    }
}

/// A parsed INFO record (one response entry under a topic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub form_id: u32,
    /// Response text string IDs (`NAM1` fields, in file order)
    /// — each resolves in `.ILSTRINGS`. Empty for script-only
    /// INFOs (no spoken line).
    pub nam1_ids: Vec<u32>,
    /// The NPC form IDs this INFO targets via GetIsID (CTDA
    /// function 72), in file order. Empty when the INFO has no
    /// GetIsID condition — it targets everyone, or is gated by
    /// other conditions M18 does not evaluate.
    pub targets: Vec<u32>,
}

/// Parse a DIAL record. `None` when the record is not a DIAL.
pub fn parse_dial(record: &Record) -> Option<Dial> {
    if &record.record_type != b"DIAL" {
        return None;
    }
    Some(Dial {
        form_id: record.form_id,
        edid: record.edid(),
        full_id: full_string_id(record),
        snam: record.field(b"SNAM").map(|f| f.as_str()),
        quest: record
            .field(b"QNAM")
            .and_then(|f| f.data.get(0..4))
            .map(|b| u32::from_le_bytes(b.try_into().unwrap())),
        priority: record
            .field(b"PNAM")
            .and_then(|f| f.data.get(0..4))
            .map(|b| f32::from_le_bytes(b.try_into().unwrap())),
    })
}

/// Parse an INFO record. `None` when the record is not an INFO.
pub fn parse_info(record: &Record) -> Option<Info> {
    if &record.record_type != b"INFO" {
        return None;
    }
    let nam1_ids = record
        .fields(b"NAM1")
        .filter_map(|f| f.data.get(0..4))
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let targets = record
        .fields(b"CTDA")
        .filter_map(|f| ctda_get_is_id(&f.data))
        .collect();
    Some(Info {
        form_id: record.form_id,
        nam1_ids,
        targets,
    })
}

/// The `FULL` field as a localized string ID: in a localized
/// plugin the FULL field is exactly 4 bytes (a u32 ID into
/// `.STRINGS`). `None` when there is no FULL field, or when it
/// is longer than 4 bytes (unlocalized inline text — not seen in
/// Skyrim.esm, but the check keeps the reader honest).
pub fn full_string_id(record: &Record) -> Option<u32> {
    let full = record.field(b"FULL")?;
    if full.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(full.data[0..4].try_into().unwrap()))
}

/// Read a CTDA condition as a GetIsID target, if it is one.
///
/// CTDA layout (32 bytes, proven from the real file):
///
/// ```text
/// 0  u8   operator
/// 1  u8[3] padding — NONZERO in some conditions (e.g.
///        [0x4e, 0x32, 0x2e]), purpose unknown, ignored
/// 4  f32  comparison value (1.0f seen)
/// 8  u16  function — THIS IS A u16, NOT A u32
/// 10 u16  unknown — nonzero sometimes (e.g. 0x18a7), ignored
/// 12 u32  param1 — the NPC form ID for GetIsID
/// 16 u32  param2
/// 20 u32  run-on
/// 24 u32  reference
/// 28 u32  unknown (0xffffffff seen)
/// ```
///
/// THE TRAP: reading the function as a u32 at offset 8 gives
/// garbage (e.g. `0x18a70048` = 413,597,768) whenever the
/// unknown u16 at offset 10 is nonzero — and that hid ~50
/// GetIsID conditions in the first probe round. Read it as a
/// u16 at offset 8: function 72 (`0x48`) = GetIsID, param1 =
/// the NPC form ID. Returns `None` for any other function, or
/// for a short field.
pub fn ctda_get_is_id(data: &[u8]) -> Option<u32> {
    if data.len() < 16 {
        return None;
    }
    let func = u16::from_le_bytes(data[8..10].try_into().unwrap());
    if func != CTDA_FUNC_GET_IS_ID {
        return None;
    }
    Some(u32::from_le_bytes(data[12..16].try_into().unwrap()))
}

/// Walk the plugin's group tree and collect the INFO records
/// under each DIAL: `TopicChildren` groups (type 7) are labelled
/// with the parent DIAL's form ID. Returns the DIAL form ID ->
/// its INFO records (in file order). DIALs with no INFO children
/// (133 in Skyrim.esm) are absent from the map.
pub fn topic_children(plugin: &Plugin) -> HashMap<u32, Vec<&Record>> {
    let mut out = HashMap::new();
    for group in &plugin.groups {
        collect_topic_children(group, &mut out);
    }
    out
}

fn collect_topic_children<'a>(group: &'a Group, out: &mut HashMap<u32, Vec<&'a Record>>) {
    if group.group_type == GroupType::TopicChildren {
        if let Some(dial_id) = group.label_form_id() {
            let infos: Vec<&Record> = group
                .records()
                .filter(|r| &r.record_type == b"INFO")
                .collect();
            if !infos.is_empty() {
                out.insert(dial_id, infos);
            }
        }
    }
    for subgroup in group.subgroups() {
        collect_topic_children(subgroup, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Field, GroupChild};

    fn field(code: &[u8; 4], data: &[u8]) -> Field {
        Field {
            field_type: *code,
            data: data.to_vec(),
        }
    }

    fn record(record_type: &[u8; 4], form_id: u32, fields: Vec<Field>) -> Record {
        Record {
            record_type: *record_type,
            flags: 0,
            form_id,
            version: 44,
            fields,
        }
    }

    /// A 32-byte CTDA with the given function (u16 @8), unknown
    /// u16 (@10), and param1 (u32 @12).
    fn ctda(func: u16, unknown: u16, param1: u32) -> Vec<u8> {
        let mut data = vec![0u8; 32];
        data[4..8].copy_from_slice(&1.0f32.to_le_bytes());
        data[8..10].copy_from_slice(&func.to_le_bytes());
        data[10..12].copy_from_slice(&unknown.to_le_bytes());
        data[12..16].copy_from_slice(&param1.to_le_bytes());
        data
    }

    #[test]
    fn parse_dial_reads_every_field() {
        let rec = record(
            b"DIAL",
            0x0005_f725,
            vec![
                field(b"EDID", b"DelphineTopic\0"),
                field(b"FULL", &378u32.to_le_bytes()),
                field(b"SNAM", b"CUST"),
                field(b"QNAM", &0x0001_cc08u32.to_le_bytes()),
                field(b"PNAM", &50.0f32.to_le_bytes()),
            ],
        );
        let dial = parse_dial(&rec).expect("a DIAL parses");
        assert_eq!(dial.form_id, 0x0005_f725);
        assert_eq!(dial.edid.as_deref(), Some("DelphineTopic"));
        assert_eq!(dial.full_id, Some(378));
        assert_eq!(dial.snam.as_deref(), Some("CUST"));
        assert_eq!(dial.quest, Some(0x0001_cc08));
        assert_eq!(dial.priority, Some(50.0));
        assert!(dial.is_player_topic());
        assert!(!dial.is_greeting());
    }

    #[test]
    fn parse_dial_greeting_and_bare_topic() {
        let greeting = record(
            b"DIAL",
            0x0001_0001,
            vec![
                field(b"SNAM", b"HELO"),
                field(b"FULL", &99u32.to_le_bytes()),
            ],
        );
        let dial = parse_dial(&greeting).expect("a HELO parses");
        assert!(dial.is_greeting());
        assert!(!dial.is_player_topic());
        // A scene topic with no EDID and no FULL still parses.
        let bare = record(b"DIAL", 0x0001_0002, vec![field(b"SNAM", b"SCEN")]);
        let dial = parse_dial(&bare).expect("a bare DIAL parses");
        assert_eq!(dial.edid, None);
        assert_eq!(dial.full_id, None);
    }

    #[test]
    fn parse_dial_rejects_non_dial() {
        let rec = record(b"INFO", 0x0001_0003, vec![]);
        assert_eq!(parse_dial(&rec), None);
    }

    #[test]
    fn parse_info_multi_response_and_targets() {
        // INFO 0x000e5080 shape: three NAM1s, one GetIsID.
        let rec = record(
            b"INFO",
            0x000e_5080,
            vec![
                field(b"NAM1", &62071u32.to_le_bytes()),
                field(b"NAM1", &63455u32.to_le_bytes()),
                field(b"NAM1", &11111u32.to_le_bytes()),
                field(b"CTDA", &ctda(72, 0, 0x0001_3478)),
            ],
        );
        let info = parse_info(&rec).expect("an INFO parses");
        assert_eq!(info.nam1_ids, vec![62071, 63455, 11111]);
        assert_eq!(info.targets, vec![0x0001_3478]);
    }

    #[test]
    fn parse_info_script_only_has_no_lines() {
        // INFO 0x00016f8c shape: VMAD + CTDA, no NAM1.
        let rec = record(
            b"INFO",
            0x0001_6f8c,
            vec![
                field(b"VMAD", &[0u8; 8]),
                field(b"CTDA", &ctda(72, 0, 0x0001_3478)),
            ],
        );
        let info = parse_info(&rec).expect("a script-only INFO parses");
        assert!(info.nam1_ids.is_empty());
        assert_eq!(info.targets, vec![0x0001_3478]);
    }

    #[test]
    fn ctda_get_is_id_reads_func_as_u16() {
        // The trap case from the first probe round: bytes
        // 8..12 read as a u32 are 0x18a70048 (413,597,768) —
        // garbage. As a u16 the function is 72, and the target
        // is still the NPC in param1.
        let data = ctda(72, 0x18a7, 0x0001_3478);
        assert_eq!(
            u32::from_le_bytes(data[8..12].try_into().unwrap()),
            413_597_768
        );
        assert_eq!(ctda_get_is_id(&data), Some(0x0001_3478));
    }

    #[test]
    fn ctda_get_is_id_rejects_other_functions_and_short_fields() {
        assert_eq!(ctda_get_is_id(&ctda(5, 0, 0x0001_3478)), None);
        assert_eq!(ctda_get_is_id(&ctda(0, 0, 0)), None);
        assert_eq!(ctda_get_is_id(&[0u8; 15]), None);
        assert_eq!(ctda_get_is_id(&[]), None);
    }

    #[test]
    fn full_string_id_needs_exactly_four_bytes() {
        let with_id = record(b"NPC_", 1, vec![field(b"FULL", &378u32.to_le_bytes())]);
        assert_eq!(full_string_id(&with_id), Some(378));
        // Unlocalized inline text is not a string ID.
        let with_text = record(b"NPC_", 2, vec![field(b"FULL", b"Delphine\0")]);
        assert_eq!(full_string_id(&with_text), None);
        let bare = record(b"NPC_", 3, vec![]);
        assert_eq!(full_string_id(&bare), None);
    }

    #[test]
    fn topic_children_maps_dial_to_infos() {
        let info_a = record(b"INFO", 0x0002_0001, vec![]);
        let info_b = record(b"INFO", 0x0002_0002, vec![]);
        let dial_group = Group {
            label: 0x0001_0001u32.to_le_bytes(),
            group_type: GroupType::TopicChildren,
            children: vec![
                GroupChild::Record(info_a),
                GroupChild::Record(info_b),
                // A non-INFO record in the group is skipped.
                GroupChild::Record(record(b"DIAL", 0x0001_0002, vec![])),
            ],
        };
        let top = Group {
            label: *b"DIAL",
            group_type: GroupType::Top,
            children: vec![GroupChild::Group(dial_group)],
        };
        let plugin = Plugin {
            header: record(b"TES4", 0, vec![]),
            groups: vec![top],
        };
        let map = topic_children(&plugin);
        assert_eq!(map.len(), 1);
        let infos = &map[&0x0001_0001];
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].form_id, 0x0002_0001);
        assert_eq!(infos[1].form_id, 0x0002_0002);
    }
}
