//! Dialogue probe round 2 (M18 research): the Nexus v1.5.23
//! DLSTRINGS did NOT resolve INFO NAM1s (round 1) — test the
//! DLSTRINGS from Patch.bsa (the game's own files, same version
//! as the ESM) instead. Also: greeting-topic identification,
//! DIAL DATA/SNAM semantics, per-NPC topic lists, and the
//! multi-response INFO structure.
//!
//! Run from the workspace root.

use dovah_bsa::BsaArchive;
use dovah_esm::{GroupChild, GroupType, Plugin, Record};
use dovah_strings::{StringsFile, StringsKind};
use std::collections::{HashMap, HashSet};

const ESM_PATH: &str = "test-data/Skyrim.esm";
const PATCH_BSA: &str = "test-data/Skyrim_-_Patch.bsa";

const DELPHINE: u32 = 0x0001_3478;
const ORGNAR: u32 = 0x0001_3479;
const EMBRY: u32 = 0x0003_550b;

fn u32_field(record: &Record, code: &[u8; 4]) -> Option<u32> {
    let f = record.field(code)?;
    if f.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
}

/// CTDA layout (proven round 1): 32 bytes, operator u8 @0,
/// comparison f32 @4, function u32 @8, param1 u32 @12, param2
/// u32 @16, run-on u32 @20. Function 72 (GetIsID) with param1 =
/// an NPC form ID is "this line is for that NPC".
fn ctda_get_is_id(ctda: &[u8]) -> Option<u32> {
    if ctda.len() < 20 {
        return None;
    }
    let func = u32::from_le_bytes(ctda[8..12].try_into().unwrap());
    if func != 72 {
        return None;
    }
    Some(u32::from_le_bytes(ctda[12..16].try_into().unwrap()))
}

fn main() {
    // --- Patch.bsa strings vs Nexus strings ---------------------
    let mut archive = BsaArchive::open(PATCH_BSA).unwrap();
    let patch_general = StringsFile::from_bytes(
        &archive.extract("strings/skyrim_english.strings").unwrap(),
        StringsKind::General,
    )
    .unwrap();
    let patch_dialogue = StringsFile::from_bytes(
        &archive.extract("strings/skyrim_english.dlstrings").unwrap(),
        StringsKind::LengthPrefixed,
    )
    .unwrap();
    println!(
        "Patch.bsa strings: STRINGS {} entries, DLSTRINGS {} entries",
        patch_general.len(),
        patch_dialogue.len()
    );
    // Round 1's failing NAM1s, against Patch DLSTRINGS.
    for nam1 in [62071u32, 10739, 40152, 30343, 63455, 40121, 46470, 50733] {
        println!(
            "  NAM1 {nam1} -> {:?}",
            patch_dialogue
                .get(nam1)
                .map(|t| t.chars().take(100).collect::<String>())
        );
    }
    // The multi-response INFO 0x000e5080's three NAM1s.
    for nam1 in [0xe7bcu32, 0x8eb4, 0xdf1e] {
        println!(
            "  INFO 0x000e5080 NAM1 0x{nam1:04x} -> {:?}",
            patch_dialogue
                .get(nam1)
                .map(|t| t.chars().take(100).collect::<String>())
        );
    }
    // FULL names against Patch STRINGS.
    for (name, id) in [("Delphine", DELPHINE), ("Orgnar", ORGNAR), ("Embry", EMBRY)] {
        let _ = (name, id);
    }

    let plugin = Plugin::open(ESM_PATH).unwrap();
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    for (name, id) in [("Delphine", DELPHINE), ("Orgnar", ORGNAR), ("Embry", EMBRY)] {
        let npc = by_form_id.get(&id).unwrap();
        let full_id = u32_field(npc, b"FULL");
        println!(
            "NPC {name}: Patch FULL -> {:?}",
            full_id.and_then(|i| patch_general.get(i))
        );
    }

    // --- INFOs grouped under DIALs ------------------------------
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

    // --- Garbage-func CTDAs: full bytes -------------------------
    let mut shown_garbage = 0;
    'outer: for infos in info_by_dial.values() {
        for info in infos {
            for ctda in info.fields(b"CTDA") {
                if ctda.data.len() < 20 {
                    continue;
                }
                let func = u32::from_le_bytes(ctda.data[8..12].try_into().unwrap());
                let param1 = u32::from_le_bytes(ctda.data[12..16].try_into().unwrap());
                if (param1 == DELPHINE || param1 == ORGNAR || param1 == EMBRY) && func != 72 {
                    println!(
                        "\ngarbage-func CTDA (INFO 0x{:08x}): len={} func={func} bytes={:02x?}",
                        info.form_id,
                        ctda.data.len(),
                        ctda.data
                    );
                    shown_garbage += 1;
                    if shown_garbage >= 3 {
                        break 'outer;
                    }
                }
            }
        }
    }

    // --- DIAL DATA + SNAM distributions -------------------------
    let dials: Vec<&Record> = plugin
        .all_records()
        .into_iter()
        .filter(|r| r.type_str() == "DIAL")
        .collect();
    let mut snam_values: HashMap<String, usize> = HashMap::new();
    let mut data_tuples: HashMap<[u8; 4], usize> = HashMap::new();
    let mut with_edid = 0;
    let mut with_full = 0;
    for dial in &dials {
        if dial.edid().is_some() {
            with_edid += 1;
        }
        if dial.field(b"FULL").is_some() {
            with_full += 1;
        }
        if let Some(snam) = dial.field(b"SNAM") {
            *snam_values
                .entry(String::from_utf8_lossy(&snam.data).into_owned())
                .or_insert(0) += 1;
        }
        if let Some(data) = dial.field(b"DATA") {
            if data.data.len() == 4 {
                let mut t = [0u8; 4];
                t.copy_from_slice(&data.data);
                *data_tuples.entry(t).or_insert(0) += 1;
            }
        }
    }
    println!(
        "\nDIALs: {} total, {with_edid} with EDID, {with_full} with FULL",
        dials.len()
    );
    println!("SNAM values: {snam_values:?}");
    let mut data_sorted: Vec<([u8; 4], usize)> = data_tuples.into_iter().collect();
    data_sorted.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!("DATA tuples (top 10): {data_sorted:?}");

    // --- Greeting topics ----------------------------------------
    println!("\nDIALs with 'Greeting' in the EDID:");
    let mut greet_count = 0;
    for dial in &dials {
        let Some(edid) = dial.edid() else {
            continue;
        };
        if !edid.contains("Greeting") {
            continue;
        }
        greet_count += 1;
        if greet_count > 6 {
            continue;
        }
        let data = dial
            .field(b"DATA")
            .map(|f| format!("{:02x?}", f.data))
            .unwrap_or_default();
        let snam = dial
            .field(b"SNAM")
            .map(|f| String::from_utf8_lossy(&f.data).into_owned())
            .unwrap_or_default();
        let quest = u32_field(dial, b"QNAM")
            .and_then(|q| by_form_id.get(&q))
            .and_then(|q| q.edid())
            .unwrap_or_default();
        let topic = u32_field(dial, b"FULL").and_then(|i| patch_general.get(i));
        println!("  {edid} DATA={data} SNAM={snam} quest={quest} topic={topic:?}");
    }
    println!("total greeting DIALs: {greet_count}");

    // --- Per-NPC topic lists (DIALs with FULL + GetIsID INFO) ---
    for (name, npc_id) in [("Delphine", DELPHINE), ("Orgnar", ORGNAR), ("Embry", EMBRY)] {
        let mut topics: Vec<(u32, String, usize)> = Vec::new();
        let mut seen: HashSet<u32> = HashSet::new();
        for (dial_id, infos) in &info_by_dial {
            if !seen.insert(*dial_id) {
                continue;
            }
            let targets_npc = infos.iter().any(|info| {
                info.fields(b"CTDA")
                    .any(|c| ctda_get_is_id(&c.data) == Some(npc_id))
            });
            if !targets_npc {
                continue;
            }
            let Some(dial) = by_form_id.get(dial_id) else {
                continue;
            };
            let Some(topic_id) = u32_field(dial, b"FULL") else {
                continue;
            };
            let Some(topic) = patch_general.get(topic_id) else {
                continue;
            };
            topics.push((*dial_id, topic.to_string(), infos.len()));
        }
        topics.sort();
        println!("\n{name}: {} topics with FULL text", topics.len());
        for (dial_id, topic, n_infos) in topics.iter().take(15) {
            println!("  0x{dial_id:08x} ({n_infos} INFOs) {topic:?}");
        }
        // Greeting lines for this NPC: INFOs under greeting DIALs.
        let mut greetings: Vec<String> = Vec::new();
        for (dial_id, infos) in &info_by_dial {
            let Some(dial) = by_form_id.get(dial_id) else {
                continue;
            };
            let is_greeting = dial.edid().map(|e| e.contains("Greeting")).unwrap_or(false);
            if !is_greeting {
                continue;
            }
            for info in infos {
                let targets_npc = info
                    .fields(b"CTDA")
                    .any(|c| ctda_get_is_id(&c.data) == Some(npc_id));
                if !targets_npc {
                    continue;
                }
                if let Some(nam1) = u32_field(info, b"NAM1") {
                    if let Some(text) = patch_dialogue.get(nam1) {
                        greetings.push(text.chars().take(120).collect());
                    }
                }
            }
        }
        println!("{name}: {} greeting lines (EDID match)", greetings.len());
        for g in greetings.iter().take(5) {
            println!("  greet: {g:?}");
        }
    }
}
