//! v0.4.6 phone-feedback probe (Bug 1 follow-up 2): dump the RAW
//! fields of the key LVLI records. The LVLD byte read by
//! lvld_probe looks like CHANCE NONE (0x32=50 on a list named
//! ...50, 0x4b=75 on a hat list), not flags — so where is the
//! "Use All" flag, and do the guard / iron set lists have it?
//!
//! Usage: cargo run -p dovah-esm --example lvli_raw_probe

use dovah_esm::Plugin;
use std::collections::HashMap;

fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() {
    let plugin = Plugin::open("test-data/Skyrim.esm").expect("open Skyrim.esm");
    let all = plugin.all_records();
    let mut by_form_id: HashMap<u32, &dovah_esm::Record> = HashMap::new();
    for record in &all {
        by_form_id.insert(record.form_id, *record);
    }
    for id in [
        0x000d_33c7u32, // OutfitListGuardWhiterun
        0x0001_3739,    // ArmorIronSetAll
        0x0010_559a,    // OutfitListHoldGuardHelmetsWhiterun
        0x000a_3fb2,    // LItemGutterSaintHat
        0x000a_3fb4,    // LItemGutterSaintBoots
        0x0001_5be2,    // LItemBanditHeavyGauntlets50
    ] {
        let rec = by_form_id[&id];
        println!(
            "\n0x{id:08x} {} ({})",
            rec.edid().unwrap_or_else(|| "?".to_string()),
            rec.type_str()
        );
        for f in &rec.fields {
            let code = String::from_utf8_lossy(&f.field_type);
            let body = if f.data.len() <= 32 {
                hex(&f.data)
            } else {
                format!("{} ... ({} bytes)", hex(&f.data[..16]), f.data.len())
            };
            println!("  {code}:{} [{}]", f.data.len(), body);
        }
    }
    // Distribution of LVLD lengths + first bytes across ALL LVLIs,
    // to see whether LVLD is 1 byte (chance none) or more.
    println!("\n--- LVLD shape across all LVLI records ---");
    let mut len_counts: HashMap<usize, usize> = HashMap::new();
    let mut first_byte: HashMap<u8, usize> = HashMap::new();
    let mut total = 0usize;
    for rec in by_form_id.values() {
        if rec.type_str() != "LVLI" {
            continue;
        }
        total += 1;
        if let Some(f) = rec.field(b"LVLD") {
            *len_counts.entry(f.data.len()).or_insert(0) += 1;
            if !f.data.is_empty() {
                *first_byte.entry(f.data[0]).or_insert(0) += 1;
            }
        }
    }
    println!("LVLI records: {total}");
    let mut lens: Vec<_> = len_counts.iter().collect();
    lens.sort();
    for (len, n) in lens {
        println!("  LVLD length {len}: {n} records");
    }
    let mut bytes: Vec<_> = first_byte.iter().collect();
    bytes.sort_by_key(|(b, _)| **b);
    println!("  LVLD first-byte values (top 12):");
    let mut by_count: Vec<_> = first_byte.iter().collect();
    by_count.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for (b, n) in by_count.iter().take(12) {
        println!("    0x{b:02x} ({b}): {n}");
    }
    let _ = bytes;
}
