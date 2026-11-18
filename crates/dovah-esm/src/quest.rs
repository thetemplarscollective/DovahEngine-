//! QUST (quest) parsing for the pause screen's journal (UI
//! milestone 1).
//!
//! Every fact here is proven from Dylan's real `Skyrim.esm` by
//! the `quest_probe` example in dovah-app (kept as the durable
//! research record). Counts below are the real file's counts:
//!
//! * QUST records: **1,811**. 1,286 with FULL, **all 1,286
//!   resolve in `.STRINGS`** (0 misses) — the quest name.
//! * DNAM: all 1,811, always 12 bytes. Bytes 0-1 are flags, byte
//!   2 is priority (0x50 = 80 on the main quests), byte 3 is
//!   the quest type — but the flags are NOT decoded here: the
//!   journal does not filter on them, and the exact bits are
//!   unverified. DNAM is not read.
//! * Stages: `INDX` + `QSDT` + `CTDA` groups in file order. A
//!   stage's journal text is its `CNAM` (4-byte string ID into
//!   **`.DLSTRINGS`** — 771/771 resolve, 0 misses). Only 203
//!   quests carry any CNAM.
//! * Objectives: `QOBJ` + `FNAM` + `NNAM` + `QSTA` groups in
//!   file order. **QOBJ is a u16** (2 bytes — the objective
//!   index: 10, 20, 30, 40 seen), NOT a u32; reading it as a
//!   u32 silently yields nothing. NNAM is the objective text
//!   string ID into **`.STRINGS`** (1,441/1,452 resolve; the 11
//!   misses are skipped by callers). Only 396 quests carry any
//!   QOBJ.
//!
//! ## INDX layout
//!
//! INDX is 4 bytes: the stage number is the **low u16** and the
//! high u16 is stage flags. MQ101 carries `[00, 00, 02, 00]`
//! (stage 0, flags 0x0002) and `[2a, 00, 00, ab]` (stage 42,
//! flags 0xab00) — reading the whole u32 as a stage gives
//! 131,072 and 2,863,616,170, which are nonsense. Callers take
//! the low u16.
//!
//! ## Field pairing is positional
//!
//! Like VTXT (LAND), a CNAM belongs to the most recent INDX and
//! an NNAM belongs to the most recent QOBJ — there is no other
//! link. MQ101's stages 150 and 180 share one CNAM text; stage
//! 900 has its own. MQ106's CNAMs appear BEFORE its first
//! INDX (4 CNAMs with no stage yet) — those are the quest's
//! un-staged log entries and parse with `stage: None`.
//!
//! ## What this module does NOT do
//!
//! Quest STATE (started/active/completed, current stage) needs
//! the quest system, which does not exist yet: this module
//! parses the static quest data only. Callers show the real
//! quest list with real names, objectives, and stage texts, and
//! mark no quest active or completed.

use crate::Record;

/// One quest stage's journal text: the stage number and the
/// `.DLSTRINGS` string ID of its log entry. `stage` is `None`
/// for log entries that appear before any INDX (MQ106's four
/// leading CNAMs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestStageText {
    pub stage: Option<u16>,
    /// `.DLSTRINGS` string ID (`CNAM`).
    pub text_id: u32,
}

/// One quest objective: its index and the `.STRINGS` string ID
/// of its text (`NNAM`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestObjective {
    pub index: u16,
    /// `.STRINGS` string ID (`NNAM`).
    pub text_id: u32,
}

/// A parsed QUST record (quest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quest {
    pub form_id: u32,
    pub edid: Option<String>,
    /// Quest name string ID (`FULL`) — resolves in `.STRINGS`.
    /// `None` when the QUST has no FULL field (525 in
    /// Skyrim.esm — system quests, not journal quests).
    pub full_id: Option<u32>,
    /// Journal texts, in file order (CNAMs paired with their
    /// preceding INDX).
    pub stage_texts: Vec<QuestStageText>,
    /// Objectives, in file order (NNAMs paired with their
    /// preceding QOBJ).
    pub objectives: Vec<QuestObjective>,
}

/// Parse a QUST record. `None` when the record is not a QUST.
pub fn parse_quest(record: &Record) -> Option<Quest> {
    if &record.record_type != b"QUST" {
        return None;
    }
    let mut stage_texts = Vec::new();
    let mut objectives = Vec::new();
    let mut current_stage: Option<u16> = None;
    let mut current_objective: Option<u16> = None;
    for field in &record.fields {
        match &field.field_type {
            b"INDX" => {
                // Stage = low u16; high u16 is stage flags.
                current_stage = field
                    .data
                    .get(0..2)
                    .map(|b| u16::from_le_bytes(b.try_into().unwrap()));
            }
            b"CNAM" => {
                if let Some(text_id) = u32_4(&field.data) {
                    stage_texts.push(QuestStageText {
                        stage: current_stage,
                        text_id,
                    });
                }
            }
            b"QOBJ" => {
                // QOBJ is a u16 (2 bytes), not a u32.
                current_objective = field
                    .data
                    .get(0..2)
                    .map(|b| u16::from_le_bytes(b.try_into().unwrap()));
            }
            b"NNAM" => {
                if let (Some(index), Some(text_id)) = (current_objective, u32_4(&field.data)) {
                    objectives.push(QuestObjective { index, text_id });
                }
            }
            _ => {}
        }
    }
    Some(Quest {
        form_id: record.form_id,
        edid: record.edid(),
        full_id: crate::dialogue::full_string_id(record),
        stage_texts,
        objectives,
    })
}

fn u32_4(data: &[u8]) -> Option<u32> {
    if data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(data[0..4].try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Field;

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

    #[test]
    fn parse_quest_mq101_shape() {
        // MQ101 0x0003372b shape: FULL, stages with CNAMs,
        // objectives with NNAMs — including the INDX flag trap
        // ([2a, 00, 00, ab] is stage 42, not 2,863,616,170).
        let rec = record(
            b"QUST",
            0x0003_372b,
            vec![
                field(b"EDID", b"MQ101\0"),
                field(b"FULL", &21205u32.to_le_bytes()),
                field(b"DNAM", &[0, 1, 0x50, 0, 0, 0, 0, 0, 1, 0, 0, 0]),
                field(b"INDX", &[0, 0, 2, 0]),
                field(b"QSDT", &[0]),
                field(b"INDX", &[150, 0, 0, 0]),
                field(b"QSDT", &[0]),
                field(b"CNAM", &10189u32.to_le_bytes()),
                field(b"INDX", &[42, 0, 0, 0xab]),
                field(b"QSDT", &[0]),
                field(b"QOBJ", &10u16.to_le_bytes()),
                field(b"FNAM", &[0, 0, 0, 0]),
                field(b"NNAM", &18004u32.to_le_bytes()),
                field(b"QOBJ", &20u16.to_le_bytes()),
                field(b"FNAM", &[0, 0, 0, 0]),
                field(b"NNAM", &10160u32.to_le_bytes()),
            ],
        );
        let quest = parse_quest(&rec).expect("a QUST parses");
        assert_eq!(quest.form_id, 0x0003_372b);
        assert_eq!(quest.edid.as_deref(), Some("MQ101"));
        assert_eq!(quest.full_id, Some(21205));
        assert_eq!(quest.stage_texts.len(), 1);
        assert_eq!(quest.stage_texts[0].stage, Some(150));
        assert_eq!(quest.stage_texts[0].text_id, 10189);
        assert_eq!(quest.objectives.len(), 2);
        assert_eq!(quest.objectives[0].index, 10);
        assert_eq!(quest.objectives[0].text_id, 18004);
        assert_eq!(quest.objectives[1].index, 20);
        assert_eq!(quest.objectives[1].text_id, 10160);
    }

    #[test]
    fn parse_quest_cnam_before_any_indx_has_no_stage() {
        // MQ106 shape: CNAMs before the first INDX parse with
        // stage None — they are the quest's un-staged log
        // entries, not stage 0's.
        let rec = record(
            b"QUST",
            0x0003_2926,
            vec![
                field(b"EDID", b"MQ106\0"),
                field(b"FULL", &14092u32.to_le_bytes()),
                field(b"CNAM", &10110u32.to_le_bytes()),
                field(b"CNAM", &5802u32.to_le_bytes()),
                field(b"INDX", &[44, 0, 0, 0]),
                field(b"CNAM", &59783u32.to_le_bytes()),
            ],
        );
        let quest = parse_quest(&rec).expect("a QUST parses");
        assert_eq!(quest.stage_texts.len(), 3);
        assert_eq!(quest.stage_texts[0].stage, None);
        assert_eq!(quest.stage_texts[1].stage, None);
        assert_eq!(quest.stage_texts[2].stage, Some(44));
    }

    #[test]
    fn parse_quest_rejects_non_qust_and_unnamed_system_quest() {
        let rec = record(b"DIAL", 0x0001_0001, vec![]);
        assert_eq!(parse_quest(&rec), None);
        // A system quest with no FULL still parses — the caller
        // filters it out of the journal, not the parser.
        let bare = record(b"QUST", 0x0001_0002, vec![field(b"EDID", b"QF_Test\0")]);
        let quest = parse_quest(&bare).expect("a bare QUST parses");
        assert_eq!(quest.full_id, None);
        assert!(quest.stage_texts.is_empty());
        assert!(quest.objectives.is_empty());
    }

    #[test]
    fn parse_quest_nnam_without_qobj_is_dropped() {
        // An NNAM with no preceding QOBJ has no objective to
        // attach to — dropped, not guessed.
        let rec = record(
            b"QUST",
            0x0001_0003,
            vec![
                field(b"FULL", &1u32.to_le_bytes()),
                field(b"NNAM", &2u32.to_le_bytes()),
            ],
        );
        let quest = parse_quest(&rec).expect("a QUST parses");
        assert!(quest.objectives.is_empty());
    }
}
