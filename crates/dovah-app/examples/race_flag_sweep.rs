//! Race-flag sweep: is there a RACE DATA flag that cleanly
//! separates creatures from people? Prints every RACE record's
//! EDID + the u32 flags at DATA+32 + bit 0 (playable?) + bit 1
//! (FaceGen head?), then cross-checks against dialogue: which
//! races have NPC_s targeted by any DIAL/INFO (GetIsID) — a
//! race with bit 1 clear but dialogue targets would be a false
//! exclusion for a talkability filter.
//!
//! Usage: cargo run -p dovah-app --example race_flag_sweep

use dovah_esm::dialogue::{parse_info, topic_children};
use dovah_esm::Plugin;
use std::collections::{HashMap, HashSet};

const ESM_PATH: &str = "test-data/Skyrim.esm";

fn field_u32(record: &dovah_esm::Record, code: &[u8; 4]) -> Option<u32> {
    let f = record.field(code)?;
    if f.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let all = plugin.all_records();
    let mut by_form_id: HashMap<u32, &dovah_esm::Record> = HashMap::new();
    for r in &all {
        by_form_id.insert(r.form_id, *r);
    }

    // NPC_s targeted by any dialogue INFO (GetIsID).
    let children = topic_children(&plugin);
    let mut talked_to: HashSet<u32> = HashSet::new();
    for infos in children.values() {
        for info_record in infos {
            if let Some(info) = parse_info(info_record) {
                for t in info.targets {
                    talked_to.insert(t);
                }
            }
        }
    }

    // Race of every NPC_, and whether that NPC_ has dialogue.
    let mut race_has_dialogue: HashMap<u32, bool> = HashMap::new();
    let mut race_npc_count: HashMap<u32, usize> = HashMap::new();
    for r in all.iter().filter(|r| r.type_str() == "NPC_") {
        let Some(race) = field_u32(r, b"RNAM") else {
            continue;
        };
        *race_npc_count.entry(race).or_default() += 1;
        if talked_to.contains(&r.form_id) {
            race_has_dialogue.insert(race, true);
        }
    }

    println!("RACE records in Skyrim.esm (flags = DATA+32 u32):");
    println!("{:<22} {:>10}  bit0 bit1  npcs  dialogue", "EDID", "flags");
    let mut races: Vec<&dovah_esm::Record> = all
        .iter()
        .filter(|r| r.type_str() == "RACE")
        .copied()
        .collect();
    races.sort_by_key(|r| r.edid().unwrap_or_default());
    let mut false_exclusions = 0;
    for race in &races {
        let Some(data) = race.field(b"DATA") else {
            continue;
        };
        if data.data.len() < 36 {
            continue;
        }
        let flags = u32::from_le_bytes(data.data[32..36].try_into().unwrap());
        let bit0 = flags & 1 != 0;
        let bit1 = flags & 2 != 0;
        let edid = race.edid().unwrap_or_default();
        let npcs = race_npc_count.get(&race.form_id).copied().unwrap_or(0);
        let dlg = race_has_dialogue
            .get(&race.form_id)
            .copied()
            .unwrap_or(false);
        println!(
            "{edid:<22} 0x{flags:08x}  {bit0:<4} {bit1:<4}  {npcs:>4}  {dlg}",
            bit0 = if bit0 { "Y" } else { "n" },
            bit1 = if bit1 { "Y" } else { "n" },
        );
        if !bit1 && dlg {
            false_exclusions += 1;
            println!("    ^^ FALSE EXCLUSION RISK: no FaceGen flag but has dialogue targets");
        }
    }
    println!("\nraces with no FaceGen flag but dialogue targets: {false_exclusions}");
}
