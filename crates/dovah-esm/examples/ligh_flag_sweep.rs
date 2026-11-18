//! M16 probe: sweep every LIGH base in Skyrim.esm and test whether
//! flags bit 12 (0x1000) is the shadow-casting flag.
//!
//! The inn's audit says: the fireplace (FarmInteriorFireLight01)
//! and the light NAMED FarmInteriorShadowLight01 both carry flags
//! 0x1009, while the 6 generic sconces carry 0x9. If bit 12
//! correlates with "Shadow" in the EDID game-wide, the rule is
//! data, not a guess — and it generalizes to every interior cell.
//!
//! Usage: cargo run -p dovah-esm --example ligh_flag_sweep

use dovah_esm::Plugin;
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut flag_counts: HashMap<u32, usize> = HashMap::new();
    let mut bit12_bases = 0usize;
    let mut bit12_non_shadow_edid = 0usize;
    let mut shadow_edid: Vec<(String, u32)> = Vec::new();
    let mut total = 0usize;
    for record in plugin.all_records() {
        if record.type_str() != "LIGH" {
            continue;
        }
        let Some(f) = record.field(b"DATA") else {
            continue;
        };
        if f.data.len() < 16 {
            continue;
        }
        total += 1;
        let flags = u32::from_le_bytes(f.data[12..16].try_into().unwrap());
        *flag_counts.entry(flags).or_insert(0) += 1;
        let edid = record.edid().unwrap_or_default().to_string();
        let has_bit12 = flags & 0x1000 != 0;
        let name_says_shadow = edid.to_lowercase().contains("shadow");
        if has_bit12 {
            bit12_bases += 1;
            if !name_says_shadow {
                bit12_non_shadow_edid += 1;
                if bit12_non_shadow_edid <= 15 {
                    println!("bit12 WITHOUT 'shadow' in EDID: {edid} flags 0x{flags:x}");
                }
            }
        }
        if name_says_shadow {
            shadow_edid.push((edid, flags));
        }
    }
    println!("---");
    println!("LIGH bases with DATA: {total}");
    let mut counts: Vec<(u32, usize)> = flag_counts.into_iter().collect();
    counts.sort_by_key(|(f, _)| *f);
    for (flags, n) in counts {
        println!("flags 0x{flags:04x}: {n}");
    }
    println!("bases with bit 12 (0x1000): {bit12_bases}");
    println!("bit12 bases without 'shadow' in EDID: {bit12_non_shadow_edid}");
    println!("EDIDs containing 'shadow': {}", shadow_edid.len());
    for (edid, flags) in &shadow_edid {
        println!("  {edid} flags 0x{flags:x}");
    }
}
