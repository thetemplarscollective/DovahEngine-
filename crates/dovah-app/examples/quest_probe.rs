//! Quest probe (pause-screen research): dump the real QUST
//! layout from Skyrim.esm and verify .strings resolution.
//! Run from the workspace root.
//!
//! Questions it answers, from the data, not the docs:
//! 1. How many QUST records are there, and which fields do they
//!    carry (sizes, order)?
//! 2. Does a QUST FULL u32 resolve in .STRINGS (quest name)?
//! 3. Where do quest stages live (INDX/QSDT/CNAM), and does
//!    CNAM resolve — in which strings file?
//! 4. Where do objectives live (QOBJ/NNAM), and does NNAM
//!    resolve — in which strings file?
//! 5. What is DNAM (flags/priority/type)?

use dovah_esm::{GroupChild, Plugin, Record};
use dovah_strings::{StringsFile, StringsKind};

const ESM_PATH: &str = "test-data/Skyrim.esm";
const STRINGS_DIR: &str = "test-data/strings/Data/Strings";

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
        let preview = if f.data.len() <= 32 {
            format!("{:02x?}", f.data)
        } else {
            format!("{:02x?}...", &f.data[..32])
        };
        println!("    {} len={} {}", f.type_str(), f.data.len(), preview);
    }
}

fn all_records(plugin: &Plugin) -> Vec<&Record> {
    fn walk<'a>(group: &'a dovah_esm::Group, out: &mut Vec<&'a Record>) {
        for child in &group.children {
            match child {
                GroupChild::Record(r) => out.push(r),
                GroupChild::Group(g) => walk(g, out),
            }
        }
    }
    let mut out = Vec::new();
    for group in &plugin.groups {
        walk(group, &mut out);
    }
    out
}

fn main() {
    let general = StringsFile::from_bytes(
        &std::fs::read(format!("{STRINGS_DIR}/Skyrim_English.STRINGS")).unwrap(),
        StringsKind::General,
    )
    .unwrap();
    let longform = StringsFile::from_bytes(
        &std::fs::read(format!("{STRINGS_DIR}/Skyrim_English.DLSTRINGS")).unwrap(),
        StringsKind::LengthPrefixed,
    )
    .unwrap();
    let dialogue = StringsFile::from_bytes(
        &std::fs::read(format!("{STRINGS_DIR}/Skyrim_English.ILSTRINGS")).unwrap(),
        StringsKind::LengthPrefixed,
    )
    .unwrap();
    println!(
        "strings: {} general, {} long-form, {} dialogue",
        general.len(),
        longform.len(),
        dialogue.len()
    );

    let plugin = Plugin::open(ESM_PATH).unwrap();
    let records = all_records(&plugin);
    let quests: Vec<&&Record> = records
        .iter()
        .filter(|r| &r.record_type == b"QUST")
        .collect();
    println!("QUST records: {}", quests.len());

    let with_full = quests.iter().filter(|r| r.field(b"FULL").is_some()).count();
    let with_dnam = quests.iter().filter(|r| r.field(b"DNAM").is_some()).count();
    let with_qobj = quests.iter().filter(|r| r.field(b"QOBJ").is_some()).count();
    let with_indx = quests.iter().filter(|r| r.field(b"INDX").is_some()).count();
    let with_cnam = quests.iter().filter(|r| r.field(b"CNAM").is_some()).count();
    println!("  with FULL: {with_full}, DNAM: {with_dnam}, QOBJ: {with_qobj}, INDX: {with_indx}, CNAM: {with_cnam}");

    // DNAM sizes.
    let mut dnam_sizes = std::collections::HashMap::new();
    for r in &quests {
        if let Some(f) = r.field(b"DNAM") {
            *dnam_sizes.entry(f.data.len()).or_insert(0usize) += 1;
        }
    }
    println!("  DNAM sizes: {dnam_sizes:?}");

    // FULL resolution: how many quest names resolve in .STRINGS?
    let mut resolved = 0usize;
    let mut unresolved = 0usize;
    for r in &quests {
        if let Some(id) = u32_field(r, b"FULL") {
            if general.get(id).is_some() {
                resolved += 1;
            } else {
                unresolved += 1;
            }
        }
    }
    println!("  FULL resolves in .STRINGS: {resolved}, misses: {unresolved}");

    // CNAM resolution: which file?
    let mut cnam_dl = 0usize;
    let mut cnam_il = 0usize;
    let mut cnam_gen = 0usize;
    let mut cnam_miss = 0usize;
    for r in &quests {
        for f in r.fields(b"CNAM") {
            if f.data.len() != 4 {
                continue;
            }
            let id = u32::from_le_bytes(f.data[0..4].try_into().unwrap());
            if longform.get(id).is_some() {
                cnam_dl += 1;
            } else if dialogue.get(id).is_some() {
                cnam_il += 1;
            } else if general.get(id).is_some() {
                cnam_gen += 1;
            } else {
                cnam_miss += 1;
            }
        }
    }
    println!(
        "  CNAM: DLSTRINGS {cnam_dl}, ILSTRINGS {cnam_il}, STRINGS {cnam_gen}, miss {cnam_miss}"
    );

    // NNAM resolution: which file?
    let mut nnam_dl = 0usize;
    let mut nnam_il = 0usize;
    let mut nnam_gen = 0usize;
    let mut nnam_miss = 0usize;
    for r in &quests {
        for f in r.fields(b"NNAM") {
            if f.data.len() != 4 {
                continue;
            }
            let id = u32::from_le_bytes(f.data[0..4].try_into().unwrap());
            if longform.get(id).is_some() {
                nnam_dl += 1;
            } else if dialogue.get(id).is_some() {
                nnam_il += 1;
            } else if general.get(id).is_some() {
                nnam_gen += 1;
            } else {
                nnam_miss += 1;
            }
        }
    }
    println!(
        "  NNAM: DLSTRINGS {nnam_dl}, ILSTRINGS {nnam_il}, STRINGS {nnam_gen}, miss {nnam_miss}"
    );

    // Dump three quests in full.
    for edid in ["MQ101", "MQ106", "FreeformRiverwood01"] {
        if let Some(q) = quests.iter().find(|r| r.edid().as_deref() == Some(edid)) {
            println!("--- {edid} ---");
            dump_fields("QUST", q);
            if let Some(id) = u32_field(q, b"FULL") {
                println!("    FULL -> {:?}", general.get(id));
            }
            // Stage log entries (INDX + CNAM pairs, in order).
            let mut stage: Option<u32> = None;
            for f in &q.fields {
                match f.type_str().as_str() {
                    "INDX" => {
                        stage = f
                            .data
                            .get(0..4)
                            .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
                    }
                    "CNAM" => {
                        let id = f
                            .data
                            .get(0..4)
                            .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
                        let text = id.and_then(|i| longform.get(i));
                        println!("    stage {stage:?} CNAM {id:?} -> {text:?}");
                    }
                    "QOBJ" => {
                        let idx = f
                            .data
                            .get(0..4)
                            .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
                        println!("    QOBJ index {idx:?}");
                    }
                    "NNAM" => {
                        let id = f
                            .data
                            .get(0..4)
                            .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
                        let text = id.and_then(|i| general.get(i));
                        println!("    NNAM {id:?} -> {text:?}");
                    }
                    _ => {}
                }
            }
        } else {
            println!("--- {edid}: NOT FOUND ---");
        }
    }
}
