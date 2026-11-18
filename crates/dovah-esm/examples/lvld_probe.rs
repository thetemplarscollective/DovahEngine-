//! v0.4.6 phone-feedback probe (Bug 1 follow-up): do LVLD flags
//! discriminate SET lists (take all entries) from CHOICE lists
//! (pick one) among the LVLIs referenced by outfits?
//!
//! The guard outfit LVLI (OutfitListGuardWhiterun) is a set —
//! cuirass + boots + shield + helmet list. But the sweep's
//! take-all rule gives some NPCs 58 pieces, which means some
//! outfit LVLIs are choice lists. LVLD byte 0 bit 0 (0x01) is
//! "calculate from all levels <= PC's level" per UESP — this
//! probe tests whether that flag is the discriminator.
//!
//! Usage: cargo run -p dovah-esm --example lvld_probe

use dovah_esm::Plugin;
use std::collections::HashMap;

fn field_id(r: &dovah_esm::Record, code: &[u8; 4]) -> Option<u32> {
    let f = r.field(code)?;
    if f.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
}

fn id_list(data: &[u8]) -> Vec<u32> {
    data.as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

fn main() {
    let plugin = Plugin::open("test-data/Skyrim.esm").expect("open Skyrim.esm");
    let all = plugin.all_records();
    let mut by_form_id: HashMap<u32, &dovah_esm::Record> = HashMap::new();
    for record in &all {
        by_form_id.insert(record.form_id, *record);
    }

    // All LVLIs referenced directly from OTFT INAM entries.
    let mut outfit_lvli: Vec<u32> = Vec::new();
    for rec in by_form_id.values() {
        if rec.type_str() != "OTFT" {
            continue;
        }
        for inam in rec.fields(b"INAM") {
            for id in id_list(&inam.data) {
                if let Some(r) = by_form_id.get(&id) {
                    if r.type_str() == "LVLI" && !outfit_lvli.contains(&id) {
                        outfit_lvli.push(id);
                    }
                }
            }
        }
    }
    outfit_lvli.sort_unstable();
    println!(
        "LVLIs referenced from OTFT INAM: {} distinct",
        outfit_lvli.len()
    );
    let mut flag_set = 0usize;
    let mut flag_clear = 0usize;
    let mut no_lvld = 0usize;
    let mut flag_rows: Vec<(u32, String, u8, usize)> = Vec::new();
    for id in &outfit_lvli {
        let rec = by_form_id[id];
        let entries = rec.fields(b"LVLO").count();
        match rec.field(b"LVLD") {
            Some(f) if !f.data.is_empty() => {
                let flags = f.data[0];
                if flags & 0x01 != 0 {
                    flag_set += 1;
                } else {
                    flag_clear += 1;
                }
                flag_rows.push((
                    *id,
                    rec.edid().unwrap_or_else(|| "?".to_string()),
                    flags,
                    entries,
                ));
            }
            _ => {
                no_lvld += 1;
                flag_rows.push((
                    *id,
                    rec.edid().unwrap_or_else(|| "?".to_string()),
                    0xff,
                    entries,
                ));
            }
        }
    }
    println!("  LVLD flag 0x01 SET: {flag_set}, CLEAR: {flag_clear}, no LVLD: {no_lvld}");
    println!("\n  --- the specific lists ---");
    for id in [0x000d_33c7u32, 0x0001_3739, 0x0010_559a] {
        if let Some((_, edid, flags, entries)) = flag_rows.iter().find(|(i, _, _, _)| *i == id) {
            println!("  0x{id:08x} {edid}: LVLD=0x{flags:02x} entries={entries}");
        } else if let Some(rec) = by_form_id.get(&id) {
            // Not directly referenced from an OTFT (nested list).
            let flags = rec
                .field(b"LVLD")
                .map(|f| if f.data.is_empty() { 0xff } else { f.data[0] })
                .unwrap_or(0xff);
            let entries = rec.fields(b"LVLO").count();
            println!(
                "  0x{id:08x} {} (nested): LVLD=0x{flags:02x} entries={entries}",
                rec.edid().unwrap_or_else(|| "?".to_string())
            );
        } else {
            println!("  0x{id:08x}: NOT FOUND",);
        }
    }
    // The 58-piece case: DA13EncAfflicted04MissileEvenTonedF's
    // outfit chain — which LVLI explodes?
    println!("\n  --- DA13EncAfflicted04MissileEvenTonedF (0x00064abd) ---");
    if let Some(npc) = by_form_id.get(&0x0006_4abd) {
        if let Some(doft) = field_id(npc, b"DOFT") {
            println!("  DOFT -> 0x{doft:08x}");
            if let Some(outfit) = by_form_id.get(&doft) {
                for inam in outfit.fields(b"INAM") {
                    for id in id_list(&inam.data) {
                        if let Some(r) = by_form_id.get(&id) {
                            println!(
                                "    INAM -> 0x{id:08x} {} ({})",
                                r.type_str(),
                                r.edid().unwrap_or_else(|| "?".to_string())
                            );
                            if r.type_str() == "LVLI" {
                                let flags = r
                                    .field(b"LVLD")
                                    .map(|f| if f.data.is_empty() { 0xff } else { f.data[0] })
                                    .unwrap_or(0xff);
                                println!(
                                    "      LVLD=0x{flags:02x} LVLO entries={}",
                                    r.fields(b"LVLO").count()
                                );
                                for lvlo in r.fields(b"LVLO").take(8) {
                                    if lvlo.data.len() < 10 {
                                        continue;
                                    }
                                    let level =
                                        i16::from_le_bytes(lvlo.data[0..2].try_into().unwrap());
                                    let item =
                                        u32::from_le_bytes(lvlo.data[4..8].try_into().unwrap());
                                    let desc = match by_form_id.get(&item) {
                                        Some(r2) => format!(
                                            "{} ({})",
                                            r2.type_str(),
                                            r2.edid().unwrap_or_else(|| "?".to_string())
                                        ),
                                        None => "NOT FOUND".to_string(),
                                    };
                                    println!("        LVLO level={level} item=0x{item:08x} {desc}");
                                }
                            }
                        }
                    }
                }
            }
        } else {
            println!("  no DOFT");
        }
    }
    // Distribution: for flag-SET vs flag-CLEAR outfit LVLIs, how
    // many LVLO entries do they hold? (A set list has a handful;
    // a choice list can hold dozens.)
    println!("\n  --- entry-count distribution by flag ---");
    let mut set_counts: Vec<usize> = flag_rows
        .iter()
        .filter(|(_, _, f, _)| *f != 0xff && *f & 0x01 != 0)
        .map(|(_, _, _, n)| *n)
        .collect();
    let mut clear_counts: Vec<usize> = flag_rows
        .iter()
        .filter(|(_, _, f, _)| *f != 0xff && *f & 0x01 == 0)
        .map(|(_, _, _, n)| *n)
        .collect();
    set_counts.sort_unstable();
    clear_counts.sort_unstable();
    let summ = |v: &Vec<usize>| -> String {
        if v.is_empty() {
            return "n=0".to_string();
        }
        format!(
            "n={} min={} median={} max={}",
            v.len(),
            v[0],
            v[v.len() / 2],
            v[v.len() - 1]
        )
    };
    println!("  flag SET (take-all?):   {}", summ(&set_counts));
    println!("  flag CLEAR (pick-one?): {}", summ(&clear_counts));
    // A few flag-CLEAR examples with many entries (choice lists).
    println!("\n  --- flag-CLEAR outfit LVLIs with >6 entries (sample) ---");
    for (id, edid, flags, entries) in flag_rows
        .iter()
        .filter(|(_, _, f, n)| *f != 0xff && *f & 0x01 == 0 && *n > 6)
        .take(10)
    {
        println!("  0x{id:08x} {edid}: LVLD=0x{flags:02x} entries={entries}");
    }
    // And flag-SET examples.
    println!("\n  --- flag-SET outfit LVLIs (sample) ---");
    for (id, edid, flags, entries) in flag_rows
        .iter()
        .filter(|(_, _, f, _)| *f != 0xff && *f & 0x01 != 0)
        .take(10)
    {
        println!("  0x{id:08x} {edid}: LVLD=0x{flags:02x} entries={entries}");
    }
}
