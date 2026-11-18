//! Book/potion description probe (inventory redesign research):
//! - Do BOOK records carry a nonzero CNAM that resolves in
//!   `.DLSTRINGS` (the detail-pane description text)?
//! - Do ALCH records carry EFID effect lists whose MGEF FULL
//!   names resolve in `.STRINGS` (the potion effect lines)?
//!
//! Run from the workspace root. Reads Patch.bsa via dovah-bsa
//! (the runtime path) — no extracted strings needed.

use dovah_bsa::BsaArchive;
use dovah_esm::{Plugin, Record};
use dovah_strings::{StringsFile, StringsKind};
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const PATCH_BSA: &str = "test-data/Skyrim_-_Patch.bsa";

fn u32_field(record: &Record, code: &[u8; 4]) -> Option<u32> {
    let f = record.field(code)?;
    if f.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
}

fn main() {
    let mut archive = BsaArchive::open(PATCH_BSA).unwrap();
    let general = StringsFile::from_bytes(
        &archive.extract("strings/skyrim_english.strings").unwrap(),
        StringsKind::General,
    )
    .unwrap();
    let longform = StringsFile::from_bytes(
        &archive.extract("strings/skyrim_english.dlstrings").unwrap(),
        StringsKind::LengthPrefixed,
    )
    .unwrap();
    println!(
        "DLSTRINGS {} entries, STRINGS {} entries",
        longform.len(),
        general.len()
    );

    let plugin = Plugin::open(ESM_PATH).unwrap();
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }

    // BOOK CNAM: count nonzero, count resolving, show 5.
    let mut books = 0;
    let mut nonzero = 0;
    let mut resolved = 0;
    let mut shown = 0;
    for record in plugin.all_records() {
        if record.type_str() != "BOOK" {
            continue;
        }
        books += 1;
        let Some(cnam) = u32_field(record, b"CNAM") else {
            continue;
        };
        if cnam == 0 {
            continue;
        }
        nonzero += 1;
        if let Some(text) = longform.get(cnam) {
            resolved += 1;
            if shown < 5 {
                let clean: String = text.chars().take(120).collect();
                println!(
                    "BOOK {} CNAM {cnam} -> {:?}",
                    record.edid().unwrap_or_default(),
                    clean
                );
                shown += 1;
            }
        }
    }
    println!("BOOK total {books}, nonzero CNAM {nonzero}, resolved in DLSTRINGS {resolved}");

    // ALCH EFID -> MGEF FULL names: show 3 potions' effect lists.
    let mut shown = 0;
    for record in plugin.all_records() {
        if record.type_str() != "ALCH" || shown >= 3 {
            continue;
        }
        let efids: Vec<u32> = record
            .fields(b"EFID")
            .filter_map(|f| {
                if f.data.len() == 4 {
                    Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
                } else {
                    None
                }
            })
            .collect();
        if efids.is_empty() {
            continue;
        }
        let names: Vec<String> = efids
            .iter()
            .map(|id| {
                by_form_id
                    .get(id)
                    .and_then(|mgef| u32_field(mgef, b"FULL"))
                    .and_then(|sid| general.get(sid))
                    .unwrap_or("?")
                    .to_string()
            })
            .collect();
        println!(
            "ALCH {} effects: {:?}",
            record.edid().unwrap_or_default(),
            names
        );
        shown += 1;
    }

    // Ground models for drops: WEAP MODL is the world mesh;
    // ARMO MOD2/MOD4 are the male/female ground models (strings,
    // unlike MODL which is an ARMA form ID on ARMO).
    let sword = by_form_id.get(&0x0001_2eb7).unwrap();
    println!(
        "WEAP IronSword MODL: {:?}",
        sword
            .field(b"MODL")
            .map(|f| String::from_utf8_lossy(&f.data).into_owned())
    );
    let cuirass = by_form_id.get(&0x0001_2e49).unwrap();
    for code in [b"MOD2", b"MOD4"] {
        println!(
            "ARMO IronCuirass {}: {:?}",
            String::from_utf8_lossy(code),
            cuirass
                .field(code)
                .map(|f| String::from_utf8_lossy(&f.data).into_owned())
        );
    }
    // How many item defs lack a ground model at all?
    let mut no_model = 0;
    let mut total = 0;
    for record in plugin.all_records() {
        let kind = record.type_str();
        let is_item = matches!(
            kind.as_str(),
            "WEAP"
                | "ARMO"
                | "MISC"
                | "ALCH"
                | "BOOK"
                | "INGR"
                | "AMMO"
                | "KEYM"
                | "SLGM"
                | "NOTE"
                | "SCRL"
        );
        if !is_item {
            continue;
        }
        total += 1;
        let has_model = if kind == "ARMO" {
            record.field(b"MOD2").is_some() || record.field(b"MOD4").is_some()
        } else {
            record.field(b"MODL").is_some()
        };
        if !has_model {
            no_model += 1;
        }
    }
    println!("item bases {total}, without a ground model {no_model}");
}
