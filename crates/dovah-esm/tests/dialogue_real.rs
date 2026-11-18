//! Real-data check for the DIAL/INFO parser: parse Dylan's
//! Skyrim.esm and verify the dialogue counts and the inn NPCs'
//! topic sets. Skips (passes) when the file is not present — the
//! synthetic tests in `dialogue.rs` are the ones that run
//! everywhere.
//!
//! The pinned counts come from the `dialogue_probe3` example
//! (see `crates/dovah-app/examples/dialogue_probe3.rs`); they
//! are exact values from the real file, not estimates.

use dovah_esm::dialogue::{parse_dial, parse_info, topic_children};
use dovah_esm::Plugin;
use std::path::PathBuf;

fn skyrim_esm() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("test-data")
        .join("Skyrim.esm")
}

/// Inn NPC form IDs (from the CELL's ACHR children).
const DELPHINE: u32 = 0x0001_3478;
const ORGNAR: u32 = 0x0001_3479;
const EMBRY: u32 = 0x0003_550b;

#[test]
fn dialogue_counts_and_inn_npc_topics() {
    let path = skyrim_esm();
    if !path.exists() {
        eprintln!(
            "skipping real-data dialogue check: {} not present",
            path.display()
        );
        return;
    }
    let plugin = Plugin::open(&path).expect("Skyrim.esm should parse");

    let dials: Vec<_> = plugin
        .all_records()
        .into_iter()
        .filter(|r| &r.record_type == b"DIAL")
        .collect();
    assert_eq!(dials.len(), 15_037, "DIAL records in Skyrim.esm");

    let infos: Vec<_> = plugin
        .all_records()
        .into_iter()
        .filter(|r| &r.record_type == b"INFO")
        .collect();
    assert_eq!(infos.len(), 31_465, "INFO records in Skyrim.esm");

    let children = topic_children(&plugin);
    assert_eq!(
        children.len(),
        14_904,
        "DIALs with INFO children (133 have none)"
    );

    // Per-NPC topic sets: a DIAL counts for an NPC when any of
    // its INFOs targets the NPC via GetIsID (CTDA func 72).
    let count_for = |npc: u32| -> (usize, usize, usize) {
        let mut any = 0;
        let mut cust_with_full = 0;
        let mut helo = 0;
        for dial_rec in &dials {
            let Some(dial) = parse_dial(dial_rec) else {
                continue;
            };
            let Some(infos) = children.get(&dial.form_id) else {
                continue;
            };
            let targets_npc = infos.iter().any(|info_rec| {
                parse_info(info_rec)
                    .map(|i| i.targets.contains(&npc))
                    .unwrap_or(false)
            });
            if !targets_npc {
                continue;
            }
            any += 1;
            if dial.is_player_topic() && dial.full_id.is_some() {
                cust_with_full += 1;
            }
            if dial.is_greeting() {
                helo += 1;
            }
        }
        (any, cust_with_full, helo)
    };

    // Delphine: the main-quest NPC — 171 DIALs, 120
    // player-selectable topics with text, 6 greetings.
    assert_eq!(
        count_for(DELPHINE),
        (171, 120, 6),
        "Delphine's topic set (any, CUST+FULL, HELO)"
    );
    // Orgnar: the innkeeper — 4 topics, 1 greeting.
    assert_eq!(
        count_for(ORGNAR),
        (10, 4, 1),
        "Orgnar's topic set (any, CUST+FULL, HELO)"
    );
    // Embry: genuinely minimal — no player topics, 1 greeting.
    assert_eq!(
        count_for(EMBRY),
        (2, 0, 1),
        "Embry's topic set (any, CUST+FULL, HELO)"
    );

    // Multi-response INFO: 0x000e5080 holds three NAM1s.
    let multi = plugin.find(0x000e_5080).expect("INFO 0x000e5080");
    let info = parse_info(multi).expect("INFO parses");
    assert_eq!(info.nam1_ids.len(), 3, "multi-response INFO");

    // Script-only INFO: 0x00016f8c has no NAM1.
    let script_only = plugin.find(0x0001_6f8c).expect("INFO 0x00016f8c");
    let info = parse_info(script_only).expect("INFO parses");
    assert!(
        info.nam1_ids.is_empty(),
        "script-only INFO has no spoken line"
    );
}
