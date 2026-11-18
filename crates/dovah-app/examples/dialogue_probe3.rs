//! Dialogue probe round 3 (M18 research): NAM1 is CRACKED —
//! INFO response text resolves in `.ILSTRINGS`, not
//! `.DLSTRINGS` (11/11 NAM1s, topic-matching lines). This round
//! uses the corrected rules end to end:
//! - CTDA function is a u16 at byte 8 (72 = GetIsID); bytes
//!   10-11 are an unknown u16 (nonzero in some conditions).
//! - INFO NAM1 -> `.ILSTRINGS` (same plugin).
//! - DIAL FULL (topic text) -> `.STRINGS`.
//! - DIAL SNAM is the subtype code: CUST = player topic,
//!   HELO = greeting, SCEN = scene, GBYE = goodbye.
//!
//! Questions: per-NPC CUST topics, HELO greetings, Embry's
//! coverage, DATA-vs-SNAM correlation, BOOK CNAM -> DLSTRINGS
//! confirmation. Run from the workspace root.

use dovah_bsa::BsaArchive;
use dovah_esm::{GroupChild, GroupType, Plugin, Record};
use dovah_strings::{StringsFile, StringsKind};
use std::collections::{HashMap, HashSet};

const ESM_PATH: &str = "test-data/Skyrim.esm";
const PATCH_BSA: &str = "test-data/Skyrim_-_Patch.bsa";

const DELPHINE: u32 = 0x0001_3478;
const ORGNAR: u32 = 0x0001_3479;
const EMBRY: u32 = 0x0003_550b;

/// GetIsID condition (function 72, u16 at byte 8): returns the
/// NPC form ID in param1 (u32 at byte 12), or None.
fn ctda_get_is_id(ctda: &[u8]) -> Option<u32> {
    if ctda.len() < 16 {
        return None;
    }
    let func = u16::from_le_bytes(ctda[8..10].try_into().unwrap());
    if func != 72 {
        return None;
    }
    Some(u32::from_le_bytes(ctda[12..16].try_into().unwrap()))
}

fn u32_field(record: &Record, code: &[u8; 4]) -> Option<u32> {
    let f = record.field(code)?;
    if f.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
}

fn snam_of(dial: &Record) -> String {
    dial.field(b"SNAM")
        .map(|f| String::from_utf8_lossy(&f.data).into_owned())
        .unwrap_or_default()
}

fn main() {
    let mut archive = BsaArchive::open(PATCH_BSA).unwrap();
    let general = StringsFile::from_bytes(
        &archive.extract("strings/skyrim_english.strings").unwrap(),
        StringsKind::General,
    )
    .unwrap();
    let lines = StringsFile::from_bytes(
        &archive.extract("strings/skyrim_english.ilstrings").unwrap(),
        StringsKind::LengthPrefixed,
    )
    .unwrap();
    let longform = StringsFile::from_bytes(
        &archive.extract("strings/skyrim_english.dlstrings").unwrap(),
        StringsKind::LengthPrefixed,
    )
    .unwrap();
    println!(
        "Patch.bsa Skyrim strings: STRINGS {} / ILSTRINGS {} / DLSTRINGS {}",
        general.len(),
        lines.len(),
        longform.len()
    );

    let plugin = Plugin::open(ESM_PATH).unwrap();
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }

    // BOOK CNAM -> DLSTRINGS confirmation.
    let mut books_checked = 0;
    for record in plugin.all_records() {
        if record.type_str() != "BOOK" {
            continue;
        }
        if let Some(cnam) = u32_field(record, b"CNAM") {
            let text = longform.get(cnam);
            println!(
                "BOOK {} CNAM {cnam} -> DLSTRINGS {:?}",
                record.edid().unwrap_or_default(),
                text.map(|t| t.chars().take(70).collect::<String>())
            );
            books_checked += 1;
            if books_checked >= 3 {
                break;
            }
        }
    }

    // INFOs grouped under DIALs.
    let mut info_by_dial: HashMap<u32, Vec<&Record>> = HashMap::new();
    fn walk<'a>(group: &'a dovah_esm::Group, info_by_dial: &mut HashMap<u32, Vec<&'a Record>>) {
        for child in &group.children {
            if let GroupChild::Group(sub) = child {
                if sub.group_type == GroupType::TopicChildren {
                    if let Some(dial_id) = sub.label_form_id() {
                        let infos: Vec<&Record> = sub
                            .children
                            .iter()
                            .filter_map(|c| match c {
                                GroupChild::Record(r) if r.type_str() == "INFO" => Some(r),
                                _ => None,
                            })
                            .collect();
                        info_by_dial.insert(dial_id, infos);
                    }
                }
                walk(sub, info_by_dial);
            }
        }
    }
    for group in &plugin.groups {
        walk(group, &mut info_by_dial);
    }

    // DATA byte1/byte2 vs SNAM correlation.
    let mut data_by_snam: HashMap<String, HashMap<(u8, u8), usize>> = HashMap::new();
    for record in plugin.all_records() {
        if record.type_str() != "DIAL" {
            continue;
        }
        let snam = snam_of(record);
        let Some(data) = record.field(b"DATA") else {
            continue;
        };
        if data.data.len() != 4 {
            continue;
        }
        *data_by_snam
            .entry(snam)
            .or_default()
            .entry((data.data[1], data.data[2]))
            .or_insert(0) += 1;
    }
    for snam in [
        "CUST", "SCEN", "HELO", "GBYE", "RUMO", "IDLE", "HIT_", "DETH",
    ] {
        if let Some(map) = data_by_snam.get(snam) {
            let mut v: Vec<((u8, u8), usize)> = map.iter().map(|(k, n)| (*k, *n)).collect();
            v.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            println!("SNAM {snam}: DATA (byte1, byte2) counts: {v:?}");
        }
    }

    // Per-NPC: CUST topics + HELO greetings + any-INFO coverage.
    for (name, npc_id) in [("Delphine", DELPHINE), ("Orgnar", ORGNAR), ("Embry", EMBRY)] {
        let mut cust_topics: Vec<(u32, String)> = Vec::new();
        let mut greetings: Vec<String> = Vec::new();
        let mut any_info_dials: HashSet<u32> = HashSet::new();
        let mut snam_coverage: HashMap<String, usize> = HashMap::new();
        for (dial_id, infos) in &info_by_dial {
            // INFOs under this DIAL that target this NPC.
            let npc_infos: Vec<&&Record> = infos
                .iter()
                .filter(|info| {
                    info.fields(b"CTDA")
                        .any(|c| ctda_get_is_id(&c.data) == Some(npc_id))
                })
                .collect();
            if npc_infos.is_empty() {
                continue;
            }
            any_info_dials.insert(*dial_id);
            let Some(dial) = by_form_id.get(dial_id) else {
                continue;
            };
            let snam = snam_of(dial);
            *snam_coverage.entry(snam.clone()).or_insert(0) += 1;
            if snam == "CUST" {
                if let Some(topic_id) = u32_field(dial, b"FULL") {
                    if let Some(topic) = general.get(topic_id) {
                        cust_topics.push((*dial_id, topic.to_string()));
                    }
                }
            }
            if snam == "HELO" {
                // First NAM1 of the first targeting INFO.
                if let Some(info) = npc_infos.first() {
                    if let Some(nam1) = u32_field(info, b"NAM1") {
                        if let Some(text) = lines.get(nam1) {
                            greetings.push(text.to_string());
                        }
                    }
                }
            }
        }
        cust_topics.sort();
        cust_topics.dedup();
        println!(
            "\n{name}: {} CUST topics, {} HELO greetings, {} DIALs with any targeting INFO",
            cust_topics.len(),
            greetings.len(),
            any_info_dials.len()
        );
        println!("  SNAM coverage: {snam_coverage:?}");
        for (dial_id, topic) in cust_topics.iter().take(20) {
            println!("  CUST 0x{dial_id:08x} {topic:?}");
        }
        for g in greetings.iter().take(5) {
            println!("  HELO {g:?}");
        }
        // First response line of the first CUST topic (end-to-end).
        if let Some((dial_id, topic)) = cust_topics.first() {
            let infos = &info_by_dial[dial_id];
            for info in infos {
                let targets = info
                    .fields(b"CTDA")
                    .any(|c| ctda_get_is_id(&c.data) == Some(npc_id));
                if !targets {
                    continue;
                }
                let nam1s: Vec<u32> = info
                    .fields(b"NAM1")
                    .filter_map(|f| {
                        if f.data.len() == 4 {
                            Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
                        } else {
                            None
                        }
                    })
                    .collect();
                let resolved: Vec<String> = nam1s
                    .iter()
                    .filter_map(|id| lines.get(*id).map(|s| s.to_string()))
                    .collect();
                println!(
                    "  end-to-end topic {topic:?} -> {} responses, first: {:?}",
                    resolved.len(),
                    resolved
                        .first()
                        .map(|s| s.chars().take(150).collect::<String>())
                );
                break;
            }
        }
    }
}
