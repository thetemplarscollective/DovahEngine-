//! Dialogue probe (M18 research): dump the real DIAL/INFO/CTDA
//! layouts from Skyrim.esm and verify .strings resolution against
//! the real files. Run from the workspace root.
//!
//! Questions it answers, from the data, not the docs:
//! 1. Does a record's FULL u32 resolve verbatim in .STRINGS
//!    (Delphine / Orgnar / Embry / an item)?
//! 2. What fields do DIAL and INFO records carry, and what are
//!    their sizes?
//! 3. How are INFOs grouped under DIALs (TopicChildren groups)?
//! 4. How does an INFO say "this line is for Delphine" — which
//!    CTDA function index, and where is the NPC form ID?
//! 5. Does an INFO NAM1 u32 resolve verbatim in .DLSTRINGS?
//! 6. What is the BSA stored path of the strings files in
//!    Patch.bsa?

use dovah_bsa::BsaArchive;
use dovah_esm::{GroupChild, GroupType, Plugin, Record};
use dovah_strings::{StringsFile, StringsKind};
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const STRINGS_DIR: &str = "test-data/strings/Data/Strings";
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

fn dump_fields(tag: &str, record: &Record) {
    println!(
        "  {tag} 0x{:08x} ({})",
        record.form_id,
        record.edid().unwrap_or_default()
    );
    for f in &record.fields {
        let preview = if f.data.len() <= 24 {
            format!("{:02x?}", f.data)
        } else {
            format!("{:02x?}...", &f.data[..24])
        };
        println!("    {} len={} {}", f.type_str(), f.data.len(), preview);
    }
}

fn main() {
    // --- 1. strings files load and resolve names ----------------
    let general = StringsFile::from_bytes(
        &std::fs::read(format!("{STRINGS_DIR}/Skyrim_English.STRINGS")).unwrap(),
        StringsKind::General,
    )
    .unwrap();
    let dialogue = StringsFile::from_bytes(
        &std::fs::read(format!("{STRINGS_DIR}/Skyrim_English.DLSTRINGS")).unwrap(),
        StringsKind::LengthPrefixed,
    )
    .unwrap();
    println!(
        "strings loaded: STRINGS {} entries, DLSTRINGS {} entries",
        general.len(),
        dialogue.len()
    );

    let plugin = Plugin::open(ESM_PATH).unwrap();
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    for (name, id) in [("Delphine", DELPHINE), ("Orgnar", ORGNAR), ("Embry", EMBRY)] {
        let npc = by_form_id.get(&id).unwrap();
        let full_id = u32_field(npc, b"FULL");
        let resolved = full_id.and_then(|i| general.get(i));
        println!("NPC {name} 0x{id:08x}: FULL id={full_id:?} -> {resolved:?}");
    }
    // An item: Iron Sword 0x00012eb7.
    let sword = by_form_id.get(&0x0001_2eb7).unwrap();
    let full_id = u32_field(sword, b"FULL");
    println!(
        "IronSword FULL id={full_id:?} -> {:?}",
        full_id.and_then(|i| general.get(i))
    );

    // --- 2. DIAL records: count + field layouts -----------------
    let dials: Vec<&Record> = plugin
        .all_records()
        .into_iter()
        .filter(|r| r.type_str() == "DIAL")
        .collect();
    println!("\nDIAL records: {}", dials.len());
    // A greeting topic and a normal topic, by EDID.
    for dial in dials.iter().take(3) {
        dump_fields("DIAL", dial);
    }
    for dial in dials.iter() {
        if dial.edid().map(|e| e.contains("Greeting")).unwrap_or(false) {
            dump_fields("DIAL greeting", dial);
            break;
        }
    }

    // --- 3. INFO children: TopicChildren groups -----------------
    let mut info_count = 0usize;
    let mut dial_with_infos = 0usize;
    let mut info_by_dial: HashMap<u32, Vec<&Record>> = HashMap::new();
    fn walk<'a>(group: &'a dovah_esm::Group, info_by_dial: &mut HashMap<u32, Vec<&'a Record>>) {
        for child in &group.children {
            match child {
                GroupChild::Group(sub) => {
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
                GroupChild::Record(_) => {}
            }
        }
    }
    for group in &plugin.groups {
        walk(group, &mut info_by_dial);
    }
    for infos in info_by_dial.values() {
        info_count += infos.len();
        if !infos.is_empty() {
            dial_with_infos += 1;
        }
    }
    println!(
        "\nINFO records under DIALs: {info_count} in {dial_with_infos} topics ({} DIALs total)",
        dials.len()
    );
    // Dump one full INFO.
    for infos in info_by_dial.values() {
        if let Some(info) = infos.first() {
            dump_fields("INFO", info);
            break;
        }
    }

    // --- 4. CTDA: which function index targets an NPC? ----------
    // Scan every INFO's CTDA fields for param1 == Delphine (or
    // Orgnar/Embry) and print the function index + full bytes.
    let mut fn_counts: HashMap<u32, usize> = HashMap::new();
    let mut shown = 0;
    for infos in info_by_dial.values() {
        for info in infos {
            for ctda in info.fields(b"CTDA") {
                if ctda.data.len() < 24 {
                    continue;
                }
                let func = u32::from_le_bytes(ctda.data[8..12].try_into().unwrap());
                let param1 = u32::from_le_bytes(ctda.data[12..16].try_into().unwrap());
                let param2 = u32::from_le_bytes(ctda.data[16..20].try_into().unwrap());
                if param1 == DELPHINE || param1 == ORGNAR || param1 == EMBRY {
                    *fn_counts.entry(func).or_insert(0) += 1;
                    if shown < 3 {
                        println!(
                            "\nCTDA targeting NPC: func={func} param1=0x{param1:08x} param2=0x{param2:08x} bytes={:02x?}",
                            ctda.data
                        );
                        shown += 1;
                    }
                }
            }
        }
    }
    println!("\nCTDA function indices with an inn-NPC param1: {fn_counts:?}");

    // --- 5. Resolve Delphine's INFO responses via DLSTRINGS -----
    let mut shown_lines = 0;
    for (dial_id, infos) in &info_by_dial {
        for info in infos {
            let targets_delphine = info.fields(b"CTDA").any(|ctda| {
                ctda.data.len() >= 16
                    && u32::from_le_bytes(ctda.data[12..16].try_into().unwrap()) == DELPHINE
            });
            if !targets_delphine {
                continue;
            }
            let nam1 = u32_field(info, b"NAM1");
            let text = nam1.and_then(|i| dialogue.get(i));
            let dial = by_form_id.get(dial_id);
            let topic = dial
                .and_then(|d| u32_field(d, b"FULL"))
                .and_then(|i| general.get(i));
            println!(
                "Delphine INFO 0x{:08x} topic={topic:?} NAM1={nam1:?} -> {:?}",
                info.form_id,
                text.map(|t| t.chars().take(120).collect::<String>())
            );
            shown_lines += 1;
            if shown_lines >= 8 {
                break;
            }
        }
        if shown_lines >= 8 {
            break;
        }
    }

    // --- 6. BSA stored path of the strings files ----------------
    let archive = BsaArchive::open(PATCH_BSA).unwrap();
    let strings_paths: Vec<&str> = archive
        .list()
        .into_iter()
        .filter(|p| p.to_lowercase().contains("strings"))
        .collect();
    println!("\nstrings paths in Patch.bsa ({}):", strings_paths.len());
    for p in strings_paths.iter().take(20) {
        println!("  {p}");
    }
}
