//! Dialogue probe round 4 (M18 research): why did Delphine's
//! first CUST topic resolve 0 responses? Dump DIAL 0x00016ee8's
//! INFOs in full, and measure the response resolve rate across
//! all three inn NPCs' CUST topics (first targeting INFO with a
//! resolvable NAM1). Run from the workspace root.

use dovah_bsa::BsaArchive;
use dovah_esm::{GroupChild, GroupType, Plugin, Record};
use dovah_strings::{StringsFile, StringsKind};
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const PATCH_BSA: &str = "test-data/Skyrim_-_Patch.bsa";

const DELPHINE: u32 = 0x0001_3478;
const ORGNAR: u32 = 0x0001_3479;
const EMBRY: u32 = 0x0003_550b;

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

fn nam1s(info: &Record) -> Vec<u32> {
    info.fields(b"NAM1")
        .filter_map(|f| {
            if f.data.len() == 4 {
                Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
            } else {
                None
            }
        })
        .collect()
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

    let plugin = Plugin::open(ESM_PATH).unwrap();
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }

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

    // The problem DIAL, in full.
    let dial_id = 0x0001_6ee8u32;
    let infos = &info_by_dial[&dial_id];
    println!("DIAL 0x{dial_id:08x} has {} INFOs:", infos.len());
    for info in infos {
        let targets = info
            .fields(b"CTDA")
            .filter_map(|c| ctda_get_is_id(&c.data))
            .collect::<Vec<_>>();
        let ids = nam1s(info);
        let il: Vec<bool> = ids.iter().map(|id| lines.get(*id).is_some()).collect();
        let dl: Vec<bool> = ids.iter().map(|id| longform.get(*id).is_some()).collect();
        let st: Vec<bool> = ids.iter().map(|id| general.get(*id).is_some()).collect();
        let ctda_count = info.fields(b"CTDA").count();
        println!(
            "  INFO 0x{:08x} targets={targets:08x?} ctda={ctda_count} nam1={ids:?} il={il:?} dl={dl:?} str={st:?}",
            info.form_id
        );
        // Field inventory for the first targeting INFO.
        if targets.contains(&DELPHINE) {
            let types: Vec<String> = info
                .fields
                .iter()
                .map(|f| format!("{}:{}", f.type_str(), f.data.len()))
                .collect();
            println!("    fields: {}", types.join(" "));
        }
    }

    // Resolve rate across all three NPCs' CUST topics.
    for (name, npc_id) in [("Delphine", DELPHINE), ("Orgnar", ORGNAR), ("Embry", EMBRY)] {
        let mut topics = 0usize;
        let mut with_line = 0usize;
        let mut no_nam1 = 0usize;
        let mut nam1_unresolved = 0usize;
        for (dial_id, infos) in &info_by_dial {
            let Some(dial) = by_form_id.get(dial_id) else {
                continue;
            };
            let snam = dial
                .field(b"SNAM")
                .map(|f| String::from_utf8_lossy(&f.data).into_owned())
                .unwrap_or_default();
            if snam != "CUST" {
                continue;
            }
            if u32_field(dial, b"FULL")
                .and_then(|i| general.get(i))
                .is_none()
            {
                continue;
            }
            let targeting: Vec<&&Record> = infos
                .iter()
                .filter(|info| {
                    info.fields(b"CTDA")
                        .any(|c| ctda_get_is_id(&c.data) == Some(npc_id))
                })
                .collect();
            if targeting.is_empty() {
                continue;
            }
            topics += 1;
            // First targeting INFO (file order) with a
            // resolvable NAM1.
            let mut found = false;
            let mut saw_nam1 = false;
            for info in targeting {
                let ids = nam1s(info);
                if !ids.is_empty() {
                    saw_nam1 = true;
                }
                if ids.iter().any(|id| lines.get(*id).is_some()) {
                    found = true;
                    break;
                }
            }
            if found {
                with_line += 1;
            } else if !saw_nam1 {
                no_nam1 += 1;
            } else {
                nam1_unresolved += 1;
            }
        }
        println!(
            "{name}: {topics} CUST topics, {with_line} with a resolvable line, {no_nam1} targeting INFOs carry no NAM1, {nam1_unresolved} NAM1s unresolvable in ILSTRINGS"
        );
    }
}
