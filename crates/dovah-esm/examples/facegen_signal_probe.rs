//! v0.4.6 phone-feedback probe (Bug 1 follow-up 3): is there a
//! RECORD-level signal for "this NPC_ has no FaceGen file"?
//!
//! Guards 01/03 (0x000274ab/0x000274ad) have no FaceGen NIF in
//! the Meshes BSAs — that is why the naked man in Dylan's
//! screenshot is headless. `resolve_npc` cannot check file
//! existence (no asset access), so a fallback needs a signal
//! readable from the record alone. Candidates: RNAM ==
//! DefaultRace, has TPLT, TPTF missing, lacks NAM9/NAMA/TINI.
//! This probe prints the confusion matrix for each.
//!
//! Part B answers the template-DOFT question: the 23 NPCs whose
//! DOFT comes only via template — what are their TPTF values?
//! (UESP: TPTF bit 0x0100 = Inventory, plausibly the outfit bit.)
//!
//! Usage: cargo run -p dovah-esm --example facegen_signal_probe

use dovah_bsa::BsaArchive;
use dovah_esm::{Plugin, Record};
use std::collections::{HashMap, HashSet};

fn field_id(r: &Record, code: &[u8; 4]) -> Option<u32> {
    let f = r.field(code)?;
    if f.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
}

fn race_has_facegen(race: &Record) -> bool {
    race.field(b"DATA")
        .and_then(|f| {
            if f.data.len() < 36 {
                return None;
            }
            Some(u32::from_le_bytes(f.data[32..36].try_into().unwrap()))
        })
        .map(|flags| flags & 0x2 != 0)
        .unwrap_or(false)
}

fn is_creature(by_form_id: &HashMap<u32, &Record>, npc: &Record) -> bool {
    let Some(race_id) = field_id(npc, b"RNAM") else {
        return false;
    };
    let Some(race) = by_form_id.get(&race_id) else {
        return false;
    };
    !race_has_facegen(race)
}

fn main() {
    let plugin = Plugin::open("test-data/Skyrim.esm").expect("open Skyrim.esm");
    let all = plugin.all_records();
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in &all {
        by_form_id.insert(record.form_id, *record);
    }

    // The set of form IDs with a FaceGen file in Meshes0/1.
    let mut facegen_ids: HashSet<u32> = HashSet::new();
    for bsa in [
        "test-data/Skyrim_-_Meshes0.bsa",
        "test-data/Skyrim_-_Meshes1.bsa",
    ] {
        let archive = BsaArchive::open(bsa).expect("open BSA");
        for p in archive.list() {
            let lower = p.to_lowercase();
            if !lower.contains("facegendata") || !lower.ends_with(".nif") {
                continue;
            }
            if let Some(stem) = lower.rsplit(['\\', '/']).next() {
                let hexpart = &stem[..stem.len().saturating_sub(4)];
                if let Ok(id) = u32::from_str_radix(hexpart, 16) {
                    facegen_ids.insert(id);
                }
            }
        }
    }
    println!("FaceGen files in Meshes0/1: {}", facegen_ids.len());

    // Part A: humanoid NPCs only (creatures never have FaceGen).
    let mut humanoids = 0usize;
    let mut no_file = 0usize;
    let mut no_file_female = 0usize;
    // signal -> (signal && no file, signal && file, !signal && no file, !signal && file)
    let mut signals: HashMap<&str, [usize; 4]> = HashMap::new();
    for name in [
        "RNAM==DefaultRace",
        "has TPLT",
        "TPTF missing",
        "lacks NAM9",
        "lacks NAMA",
        "lacks TINI",
    ] {
        signals.insert(name, [0; 4]);
    }
    for rec in by_form_id.values() {
        if rec.type_str() != "NPC_" {
            continue;
        }
        if is_creature(&by_form_id, rec) {
            continue;
        }
        if rec.form_id >> 24 != 0 {
            continue; // not mod index 0: no FaceGen path in our engine
        }
        humanoids += 1;
        let has_file = facegen_ids.contains(&rec.form_id);
        if !has_file {
            no_file += 1;
            let female = rec
                .field(b"ACBS")
                .and_then(|f| f.data.first().map(|b| b & 1 == 1))
                .unwrap_or(false);
            if female {
                no_file_female += 1;
            }
        }
        let checks: [(&str, bool); 6] = [
            (
                "RNAM==DefaultRace",
                field_id(rec, b"RNAM") == Some(0x0000_0019),
            ),
            ("has TPLT", rec.field(b"TPLT").is_some()),
            ("TPTF missing", rec.field(b"TPTF").is_none()),
            ("lacks NAM9", rec.field(b"NAM9").is_none()),
            ("lacks NAMA", rec.field(b"NAMA").is_none()),
            ("lacks TINI", rec.field(b"TINI").is_none()),
        ];
        for (name, signal) in checks {
            let slot = match (signal, has_file) {
                (true, false) => 0,
                (true, true) => 1,
                (false, false) => 2,
                (false, true) => 3,
            };
            signals.get_mut(name).unwrap()[slot] += 1;
        }
    }
    println!("Humanoid NPC_ (mod index 0): {humanoids}");
    println!("  without a FaceGen file: {no_file} (female: {no_file_female})");
    println!("\n  signal                  | sig+noFile sig+file !sig+noFile !sig+file");
    for (name, c) in &signals {
        println!(
            "  {name:<24} | {:>10} {:>9} {:>11} {:>9}",
            c[0], c[1], c[2], c[3]
        );
    }
    // The guards, for the record.
    println!("\n  the Riverwood guards:");
    for id in [0x0002_74abu32, 0x0002_74ac, 0x0002_74ad] {
        let rec = by_form_id[&id];
        println!(
            "    0x{id:08x} {}: file={} RNAM=0x{:08x} TPLT={} TPTF={} NAM9={}",
            rec.edid().unwrap_or_else(|| "?".to_string()),
            facegen_ids.contains(&id),
            field_id(rec, b"RNAM").unwrap_or(0),
            rec.field(b"TPLT").is_some(),
            rec.field(b"TPTF").is_some(),
            rec.field(b"NAM9").is_some(),
        );
    }

    // Part B: the 23 DOFT-only-via-template NPCs and their TPTF.
    println!("\n--- Part B: DOFT only via template ---");
    let mut tptf_present = 0usize;
    let mut tptf_missing = 0usize;
    let mut tptf_values: HashMap<u16, usize> = HashMap::new();
    let mut via_template: Vec<(u32, String, Option<u16>)> = Vec::new();
    for rec in by_form_id.values() {
        if rec.type_str() != "NPC_" {
            continue;
        }
        if rec.field(b"TPLT").is_none() {
            continue;
        }
        match rec.field(b"TPTF") {
            Some(f) if f.data.len() >= 2 => {
                tptf_present += 1;
                let v = u16::from_le_bytes(f.data[0..2].try_into().unwrap());
                *tptf_values.entry(v).or_insert(0) += 1;
            }
            _ => tptf_missing += 1,
        }
        if rec.field(b"DOFT").is_some() {
            continue;
        }
        // Walk the template chain for a DOFT.
        let mut cur = field_id(rec, b"TPLT");
        let mut depth = 0;
        let mut found = false;
        while let Some(tid) = cur {
            if depth > 8 {
                break;
            }
            depth += 1;
            let Some(t) = by_form_id.get(&tid) else { break };
            if t.field(b"DOFT").is_some() {
                found = true;
                break;
            }
            cur = field_id(t, b"TPLT");
        }
        if found {
            let tptf = rec.field(b"TPTF").and_then(|f| {
                if f.data.len() >= 2 {
                    Some(u16::from_le_bytes(f.data[0..2].try_into().unwrap()))
                } else {
                    None
                }
            });
            via_template.push((
                rec.form_id,
                rec.edid().unwrap_or_else(|| "?".to_string()),
                tptf,
            ));
        }
    }
    println!("NPCs with TPLT: TPTF present={tptf_present} missing={tptf_missing}");
    let mut vals: Vec<_> = tptf_values.iter().collect();
    vals.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    println!("  top TPTF values:");
    for (v, n) in vals.iter().take(8) {
        println!("    0x{v:04x}: {n}");
    }
    println!("NPCs with DOFT only via template: {}", via_template.len());
    for (id, edid, tptf) in &via_template {
        println!(
            "  0x{id:08x} {edid}: TPTF={}",
            tptf.map(|v| format!("0x{v:04x}"))
                .unwrap_or_else(|| "MISSING".to_string())
        );
    }
}
