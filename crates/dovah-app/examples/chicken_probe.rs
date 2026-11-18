//! Chicken-dialogue probe: list every ACHR ref the app loads in the
//! Riverwood 3x3 block (centre (5,-11), exactly as the game loads
//! it after the inn-door transition) with base type, base EDID,
//! and Bevy position — and sort by distance from the door-arrival
//! spawn, to show which actors the nearest-in-range dialogue
//! picker can hand the player.
//!
//! Usage: cargo run -p dovah-app --example chicken_probe

use dovah_app::cell::{load_exterior, SpawnPoint};
use dovah_esm::Plugin;

const ESM_PATH: &str = "test-data/Skyrim.esm";
/// Skyrim units per metre (coords::SKYRIM_UNITS_PER_METER —
/// the coords module is private, so the probe uses the value).
const UNITS_PER_METER: f32 = 70.0;

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let all = plugin.all_records();
    // The inn door's XTEL arrival, from riverwood_probe's
    // findings — the spawn the player gets on the door
    // transition. (Recomputed here from the XTEL record so the
    // probe stands alone.)
    let spawn = door_arrival(&plugin);
    let data = load_exterior(&plugin, (5, -11), spawn).expect("load Riverwood");
    let spawn_pos: [f32; 3] = spawn.map(|s| s.position).unwrap_or([0.0, 0.0, 0.0]);
    println!(
        "Riverwood: {} objects, spawn at Skyrim ({:.0}, {:.0}, {:.0})",
        data.objects.len(),
        spawn_pos[0],
        spawn_pos[1],
        spawn_pos[2]
    );
    let mut actors: Vec<_> = data.objects.iter().filter(|o| o.is_actor).collect();
    actors.sort_by(|a, b| {
        let da = dist2(a, &spawn_pos);
        let db = dist2(b, &spawn_pos);
        da.partial_cmp(&db).unwrap()
    });
    println!("ACHR refs in the loaded block: {}", actors.len());
    for o in &actors {
        let d = dist2(o, &spawn_pos).sqrt();
        println!(
            "  ref 0x{:08x} base 0x{:08x} {:<5} {:<28} pos ({:8.0}, {:8.0}, {:7.0}) dist from spawn {:6.1} m  is_creature={} parts={}",
            o.ref_form_id, o.base_form_id, o.base_type, o.base_edid, o.position[0], o.position[1], o.position[2], d, o.is_creature, o.actor_models.len()
        );
    }
    let creatures = actors.iter().filter(|o| o.base_type != "NPC_").count();
    println!("creatures (non-NPC_ base) among them: {creatures}");

    // ---- Part 2: what each actor RENDERS as (actor_models is
    // the resolve_npc output, carried on the PlacedObject) and
    // its race, with the RACE record's fields — hunting the
    // creature/person discriminator.
    let mut by_form_id: std::collections::HashMap<u32, &dovah_esm::Record> =
        std::collections::HashMap::new();
    for record in &all {
        by_form_id.insert(record.form_id, *record);
    }
    println!("\n--- render parts + race per actor ---");
    for o in &actors {
        let race_id = by_form_id
            .get(&o.base_form_id)
            .and_then(|b| b.field(b"RNAM"))
            .and_then(|f| {
                if f.data.len() == 4 {
                    Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
                } else {
                    None
                }
            });
        let race_edid = race_id
            .and_then(|id| by_form_id.get(&id))
            .and_then(|r| r.edid())
            .unwrap_or_else(|| "?".to_string());
        println!(
            "  ref 0x{:08x} {:<28} race 0x{:08x} {:<14} parts: {:?}",
            o.ref_form_id,
            o.base_edid,
            race_id.unwrap_or(0),
            race_edid,
            o.actor_models
        );
    }
    println!("\n--- RACE record fields (DATA hex) ---");
    let mut seen_races: Vec<u32> = Vec::new();
    for o in &actors {
        let Some(race_id) = by_form_id
            .get(&o.base_form_id)
            .and_then(|b| b.field(b"RNAM"))
            .and_then(|f| {
                if f.data.len() == 4 {
                    Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
                } else {
                    None
                }
            })
        else {
            continue;
        };
        if seen_races.contains(&race_id) {
            continue;
        }
        seen_races.push(race_id);
        let Some(race) = by_form_id.get(&race_id) else {
            continue;
        };
        let fields: Vec<String> = race
            .fields
            .iter()
            .map(|f| {
                format!(
                    "{}:{}",
                    String::from_utf8_lossy(&f.field_type),
                    f.data.len()
                )
            })
            .collect();
        let data_hex = race
            .field(b"DATA")
            .map(|f| {
                f.data
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_else(|| "-".to_string());
        println!(
            "  0x{:08x} {:<14} [{}]",
            race_id,
            race.edid().unwrap_or_default(),
            fields.join(" ")
        );
        println!("      DATA: {data_hex}");
    }
    // Humanoid races for comparison.
    println!("\n--- humanoid races for comparison ---");
    for edid in ["NordRace", "BretonRace", "ImperialRace", "RedguardRace"] {
        let Some(race) = all
            .iter()
            .find(|r| r.type_str() == "RACE" && r.edid().as_deref() == Some(edid))
        else {
            continue;
        };
        let data_hex = race
            .field(b"DATA")
            .map(|f| {
                f.data
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_else(|| "-".to_string());
        println!("  0x{:08x} {edid:<14} DATA: {data_hex}", race.form_id);
    }

    // ---- Part 3: where are the villagers? Find ACHR refs for
    // Riverwood's named NPCs and say which cell files them.
    println!("\n--- villager refs ---");
    let villagers = [
        "Alvor",
        "Gerdur",
        "Hod",
        "Faendal",
        "Sven",
        "CamillaValerius",
        "LucanValerius",
        "Frodnar",
        "Dorthe",
        "Embry",
        "Sigrid",
        "Ralof",
    ];
    // Reverse map: ref form ID -> parent cell form ID.
    let index = dovah_app::cell::CellIndex::build(&plugin);
    let mut parent_of: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for cell in all.iter().filter(|r| r.type_str() == "CELL") {
        for child in index.children_of(cell.form_id) {
            parent_of.insert(child.form_id, cell.form_id);
        }
    }
    for edid in villagers {
        let Some(npc) = all
            .iter()
            .find(|r| r.type_str() == "NPC_" && r.edid().as_deref() == Some(edid))
        else {
            println!("  {edid}: no NPC_ record");
            continue;
        };
        let refs: Vec<&&dovah_esm::Record> = all
            .iter()
            .filter(|r| {
                r.type_str() == "ACHR"
                    && r.field(b"NAME")
                        .map(|f| {
                            f.data.len() == 4
                                && u32::from_le_bytes(f.data[0..4].try_into().unwrap())
                                    == npc.form_id
                        })
                        .unwrap_or(false)
            })
            .collect();
        if refs.is_empty() {
            println!(
                "  {edid} (0x{:08x}): NO ACHR ref in the plugin",
                npc.form_id
            );
            continue;
        }
        for r in refs {
            let parent = parent_of.get(&r.form_id).copied().unwrap_or(0);
            let parent_edid = by_form_id
                .get(&parent)
                .and_then(|c| c.edid())
                .unwrap_or_else(|| "?".to_string());
            let pos = r.field(b"DATA").and_then(|f| {
                if f.data.len() == 24 {
                    Some((
                        f32::from_le_bytes(f.data[0..4].try_into().unwrap()),
                        f32::from_le_bytes(f.data[4..8].try_into().unwrap()),
                        f32::from_le_bytes(f.data[8..12].try_into().unwrap()),
                    ))
                } else {
                    None
                }
            });
            let has_xesp = r.field(b"XESP").is_some();
            println!(
                "  {edid} ref 0x{:08x} parent cell 0x{:08x} ({parent_edid}) pos {:?} flags 0x{:08x} XESP={has_xesp}",
                r.form_id, parent, pos, r.flags
            );
        }
    }
}

/// Horizontal distance in metres between an object and a
/// Skyrim-space point (Skyrim X/Y are the horizontal plane).
fn dist2(o: &dovah_app::cell::PlacedObject, spawn: &[f32; 3]) -> f32 {
    let dx = (o.position[0] - spawn[0]) / UNITS_PER_METER;
    let dy = (o.position[1] - spawn[1]) / UNITS_PER_METER;
    dx * dx + dy * dy
}

/// The inn door's XTEL arrival: find the Sleeping Giant Inn's
/// load door (REFR with XTEL in the inn cell) and take its
/// destination position.
fn door_arrival(plugin: &Plugin) -> Option<SpawnPoint> {
    let inn = plugin
        .all_records()
        .iter()
        .find(|r| {
            r.type_str() == "CELL" && r.edid().as_deref() == Some("RiverwoodSleepingGiantInn")
        })
        .copied()?;
    let index = dovah_app::cell::CellIndex::build(plugin);
    for r in index.children_of(inn.form_id) {
        let Some(xtel) = r.field(b"XTEL") else {
            continue;
        };
        if xtel.data.len() < 24 {
            continue;
        }
        let f = |i: usize| f32::from_le_bytes(xtel.data[i * 4..i * 4 + 4].try_into().unwrap());
        // XTEL: door form id (4) + position (12) + rotation (12).
        return Some(SpawnPoint {
            position: [f(1), f(2), f(3)],
            yaw: f(6),
        });
    }
    None
}
