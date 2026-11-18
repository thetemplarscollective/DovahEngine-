//! v0.4.6 phone-feedback probe (Bug 1, the naked guard): trace the
//! outfit chain for the Riverwood Imperial guards end to end and
//! find the exact link that yields zero outfit pieces.
//!
//! Part 1: list the Riverwood actors (as the game loads them) with
//! their resolved part counts, to identify the guards.
//! Part 2: for each guard + the inn NPCs (controls), dump the
//! NPC_ fields, follow the TPLT template chain, resolve DOFT ->
//! OTFT -> INAM entries (with types — ARMO vs LVLI), each ARMO's
//! MODL -> ARMA -> MOD2/MOD3, and check the FaceGen NIF exists
//! in the Meshes BSAs.
//! Part 3: sweep all NPC_ records: TPLT usage, DOFT direct vs
//! via-template, INAM entry types, and how many NPCs get 0 outfit
//! pieces under the current chain vs the fixed chain (template
//! walk + flag-aware LVLI resolution, mirroring npc.rs).
//! Part 1 also prints actor positions (Skyrim units + Bevy
//! metres) so --camera shots can frame the chicken and guards.
//!
//! Usage: cargo run -p dovah-app --example guard_outfit_probe

use dovah_app::cell::{load_exterior, SpawnPoint};
use dovah_bsa::BsaArchive;
use dovah_esm::{Plugin, Record};
use std::collections::{HashMap, HashSet};

const ESM_PATH: &str = "test-data/Skyrim.esm";

fn field_id(record: &Record, code: &[u8; 4]) -> Option<u32> {
    let f = record.field(code)?;
    if f.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
}

fn field_u16(record: &Record, code: &[u8; 4]) -> Option<u16> {
    let f = record.field(code)?;
    if f.data.len() < 2 {
        return None;
    }
    Some(u16::from_le_bytes(f.data[0..2].try_into().unwrap()))
}

fn id_list(data: &[u8]) -> Vec<u32> {
    data.as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

/// Follow the TPLT chain from an NPC_, returning the chain of
/// (form_id, edid) pairs, template-first order (the NPC itself is
/// element 0). Caps at depth 6 to bound cycles.
fn template_chain(by_form_id: &HashMap<u32, &Record>, start: u32) -> Vec<(u32, String)> {
    let mut chain = Vec::new();
    let mut cur = Some(start);
    let mut seen = HashSet::new();
    while let Some(id) = cur {
        if !seen.insert(id) || chain.len() >= 6 {
            break;
        }
        let Some(rec) = by_form_id.get(&id) else {
            break;
        };
        chain.push((id, rec.edid().unwrap_or_else(|| "?".to_string())));
        cur = field_id(rec, b"TPLT");
    }
    chain
}

/// The outfit form ID an NPC_ effectively wears: its own DOFT, else
/// the first DOFT found walking up its TPLT chain. Returns
/// (outfit_id, from_form_id) — the record the DOFT came from.
fn effective_outfit(by_form_id: &HashMap<u32, &Record>, npc_id: u32) -> Option<(u32, u32)> {
    for (id, _) in template_chain(by_form_id, npc_id) {
        let rec = by_form_id.get(&id)?;
        if let Some(doft) = field_id(rec, b"DOFT") {
            return Some((doft, id));
        }
    }
    None
}

/// Resolve one OTFT's INAM entries to ARMO form IDs, following
/// the SHIPPED rule (npc.rs `resolve_outfit_lvli`, v0.4.6 phone
/// feedback): an LVLI entry resolves by its LVLF flags — Use All
/// (0x04) contributes ALL entries at or below player level 1,
/// otherwise ONE deterministic pick (hash of list + NPC, the M17
/// container rule seeded by the NPC); Chance None (LVLD) is
/// honored on pick-one lists, ignored on Use-All lists.
/// Recurses into nested lists, each applying its own flag
/// (depth cap 4).
fn outfit_armor_ids(
    by_form_id: &HashMap<u32, &Record>,
    outfit_id: u32,
    npc_form_id: u32,
) -> Vec<u32> {
    let Some(outfit) = by_form_id.get(&outfit_id) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for inam in outfit.fields(b"INAM") {
        for id in id_list(&inam.data) {
            match by_form_id.get(&id) {
                Some(rec) if rec.type_str() == "ARMO" => out.push(id),
                Some(rec) if rec.type_str() == "LVLI" => {
                    out.extend(resolve_outfit_lvli(by_form_id, id, npc_form_id, 0));
                }
                _ => {}
            }
        }
    }
    out
}

/// The flag-aware LVLI rule, mirroring npc.rs exactly (same
/// hash, same chance-none roll) so the sweep counts what the
/// shipped code resolves.
fn resolve_outfit_lvli(
    by_form_id: &HashMap<u32, &Record>,
    lvli_id: u32,
    npc_form_id: u32,
    depth: u32,
) -> Vec<u32> {
    if depth > 4 {
        return Vec::new();
    }
    let Some(list) = by_form_id.get(&lvli_id) else {
        return Vec::new();
    };
    if list.type_str() != "LVLI" {
        return Vec::new();
    }
    let eligible: Vec<u32> = list
        .fields(b"LVLO")
        .filter_map(|lvlo| {
            if lvlo.data.len() < 8 {
                return None;
            }
            let level = i16::from_le_bytes(lvlo.data[0..2].try_into().unwrap());
            if level > 1 {
                return None;
            }
            Some(u32::from_le_bytes(lvlo.data[4..8].try_into().unwrap()))
        })
        .collect();
    if eligible.is_empty() {
        return Vec::new();
    }
    let use_all = list
        .field(b"LVLF")
        .and_then(|f| f.data.first())
        .map(|b| b & 0x04 != 0)
        .unwrap_or(false);
    if use_all {
        let mut out = Vec::new();
        for item in eligible {
            if by_form_id
                .get(&item)
                .map(|r| r.type_str() == "LVLI")
                .unwrap_or(false)
            {
                out.extend(resolve_outfit_lvli(
                    by_form_id,
                    item,
                    npc_form_id,
                    depth + 1,
                ));
            } else {
                out.push(item);
            }
        }
        return out;
    }
    let hash = lvli_id
        .wrapping_mul(2_654_435_761)
        .wrapping_add(npc_form_id.wrapping_mul(40_503))
        .wrapping_add(depth.wrapping_mul(97));
    let chance_none = list
        .field(b"LVLD")
        .and_then(|f| f.data.first())
        .copied()
        .unwrap_or(0) as u32;
    if hash % 100 < chance_none {
        return Vec::new();
    }
    let picked = eligible[(hash as usize) % eligible.len()];
    if by_form_id
        .get(&picked)
        .map(|r| r.type_str() == "LVLI")
        .unwrap_or(false)
    {
        resolve_outfit_lvli(by_form_id, picked, npc_form_id, depth + 1)
    } else {
        vec![picked]
    }
}

/// Count the outfit MODELS the current resolve_npc chain yields for
/// an NPC_ (own DOFT only, ARMO entries only, race-matched ARMA
/// with a MOD2/MOD3 for the NPC's sex) — the M12 chain, replicated
/// so the sweep can compare old vs fixed without the private npc
/// module.
fn current_chain_piece_count(by_form_id: &HashMap<u32, &Record>, npc: &Record) -> usize {
    let female = npc
        .field(b"ACBS")
        .and_then(|f| f.data.first().map(|b| b & 1 == 1))
        .unwrap_or(false);
    let race = field_id(npc, b"RNAM").unwrap_or(0x0000_0019);
    let Some(outfit_id) = field_id(npc, b"DOFT") else {
        return 0;
    };
    let Some(outfit) = by_form_id.get(&outfit_id) else {
        return 0;
    };
    let mut count = 0;
    for inam in outfit.fields(b"INAM") {
        for armo_id in id_list(&inam.data) {
            if armor_model(by_form_id, armo_id, race, female).is_some() {
                count += 1;
            }
        }
    }
    count
}

/// The fixed chain: effective DOFT (template walk) + flag-aware
/// LVLI resolution (the shipped npc.rs rule).
fn fixed_chain_piece_count(by_form_id: &HashMap<u32, &Record>, npc: &Record) -> usize {
    let female = npc
        .field(b"ACBS")
        .and_then(|f| f.data.first().map(|b| b & 1 == 1))
        .unwrap_or(false);
    let race = field_id(npc, b"RNAM").unwrap_or(0x0000_0019);
    let Some((outfit_id, _)) = effective_outfit(by_form_id, npc.form_id) else {
        return 0;
    };
    outfit_armor_ids(by_form_id, outfit_id, npc.form_id)
        .iter()
        .filter(|id| armor_model(by_form_id, **id, race, female).is_some())
        .count()
}

/// ARMO -> race-matched ARMA -> MOD2/MOD3 (the M12 armor_model,
/// replicated for the sweep).
fn armor_model(
    by_form_id: &HashMap<u32, &Record>,
    armo_id: u32,
    race: u32,
    female: bool,
) -> Option<String> {
    let armo = by_form_id.get(&armo_id)?;
    if armo.type_str() != "ARMO" {
        return None;
    }
    let armas: Vec<&Record> = armo
        .fields(b"MODL")
        .filter_map(|f| {
            if f.data.len() != 4 {
                return None;
            }
            let id = u32::from_le_bytes(f.data[0..4].try_into().unwrap());
            by_form_id.get(&id).copied()
        })
        .filter(|a| a.type_str() == "ARMA")
        .collect();
    if armas.is_empty() {
        return None;
    }
    let mut picked = None;
    for wanted in [race, 0x0000_0019] {
        if let Some(a) = armas.iter().find(|a| field_id(a, b"RNAM") == Some(wanted)) {
            picked = Some(*a);
            break;
        }
    }
    let arma = picked.unwrap_or(armas[0]);
    let code = if female { b"MOD3" } else { b"MOD2" };
    let model = arma.field(code)?.as_str();
    if model.is_empty() {
        None
    } else {
        Some(model)
    }
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let all = plugin.all_records();
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in &all {
        by_form_id.insert(record.form_id, *record);
    }
    println!("records indexed: {}", by_form_id.len());

    // ---- Part 1: Riverwood actors, as the game loads them.
    // Positions in Skyrim units + Bevy metres (Bevy = x/70,
    // z/70, -y/70) so --camera shots can frame the chicken and
    // the guards.
    let spawn = door_arrival(&plugin);
    if let Some(s) = spawn {
        println!(
            "inn-door spawn: Skyrim [{:.0}, {:.0}, {:.0}] yaw {:.3} -> Bevy [{:.1}, {:.1}, {:.1}]",
            s.position[0],
            s.position[1],
            s.position[2],
            s.yaw,
            s.position[0] / 70.0,
            s.position[2] / 70.0,
            -s.position[1] / 70.0
        );
    }
    let data = load_exterior(&plugin, (5, -11), spawn).expect("load Riverwood");
    let mut actors: Vec<_> = data.objects.iter().filter(|o| o.is_actor).collect();
    actors.sort_by_key(|o| o.ref_form_id);
    println!(
        "\n--- Part 1: Riverwood actors ({} total) ---",
        actors.len()
    );
    for o in &actors {
        println!(
            "  ref 0x{:08x} base 0x{:08x} {:<5} {:<28} creature={} parts={} Skyrim [{:.0}, {:.0}, {:.0}] Bevy [{:.1}, {:.1}, {:.1}]",
            o.ref_form_id,
            o.base_form_id,
            o.base_type,
            o.base_edid,
            o.is_creature,
            o.actor_models.len(),
            o.position[0],
            o.position[1],
            o.position[2],
            o.position[0] / 70.0,
            o.position[2] / 70.0,
            -o.position[1] / 70.0
        );
    }

    // ---- Part 2: chain trace for the guards + inn controls.
    // FaceGen existence set from the Meshes BSAs.
    let mut facegen_present: HashSet<String> = HashSet::new();
    for bsa in [
        "test-data/Skyrim_-_Meshes0.bsa",
        "test-data/Skyrim_-_Meshes1.bsa",
    ] {
        match BsaArchive::open(bsa) {
            Ok(archive) => {
                for stored in archive.list() {
                    facegen_present.insert(stored.replace('\\', "/").to_lowercase());
                }
                println!("indexed {bsa}: {} files", archive.len());
            }
            Err(e) => println!("warning: could not open {bsa}: {e}"),
        }
    }

    let mut trace_ids: Vec<u32> = actors
        .iter()
        .filter(|o| !o.is_creature && o.base_type == "NPC_")
        .map(|o| o.base_form_id)
        .collect();
    trace_ids.sort_unstable();
    trace_ids.dedup();
    // Inn controls (they resolve fully).
    for control in [0x0001_3478u32, 0x0001_3479, 0x0003_550b] {
        if !trace_ids.contains(&control) {
            trace_ids.push(control);
        }
    }
    println!("\n--- Part 2: outfit chain trace ---");
    for npc_id in trace_ids {
        trace_npc(&by_form_id, &facegen_present, npc_id);
    }

    // ---- Part 3: sweep all NPC_ records.
    println!("\n--- Part 3: sweep over all NPC_ records ---");
    sweep(&by_form_id);
}

fn trace_npc(by_form_id: &HashMap<u32, &Record>, facegen_present: &HashSet<String>, npc_id: u32) {
    let Some(npc) = by_form_id.get(&npc_id) else {
        println!("\n0x{npc_id:08x}: NOT FOUND");
        return;
    };
    let edid = npc.edid().unwrap_or_else(|| "?".to_string());
    println!("\nNPC_ 0x{npc_id:08x} ({edid})");
    let field_codes: Vec<String> = npc
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
    println!("  fields: {}", field_codes.join(" "));
    if let Some(tplt) = field_id(npc, b"TPLT") {
        let tplt_edid = by_form_id
            .get(&tplt)
            .and_then(|r| r.edid())
            .unwrap_or_else(|| "?".to_string());
        println!("  TPLT -> 0x{tplt:08x} ({tplt_edid})");
        if let Some(flags) = field_u16(npc, b"TPTF") {
            println!("  TPTF flags: 0x{flags:04x}");
        } else {
            println!("  TPTF: MISSING");
        }
    } else {
        println!("  TPLT: none");
    }
    let chain = template_chain(by_form_id, npc_id);
    if chain.len() > 1 {
        let names: Vec<String> = chain
            .iter()
            .map(|(id, e)| format!("0x{id:08x} ({e})"))
            .collect();
        println!("  template chain: {}", names.join(" -> "));
    }
    let race_id = field_id(npc, b"RNAM");
    let race_edid = race_id
        .and_then(|id| by_form_id.get(&id))
        .and_then(|r| r.edid())
        .unwrap_or_else(|| "?".to_string());
    let female = npc
        .field(b"ACBS")
        .and_then(|f| f.data.first().map(|b| b & 1 == 1))
        .unwrap_or(false);
    println!(
        "  race: 0x{:08x} ({race_edid}) female={female}",
        race_id.unwrap_or(0)
    );
    match field_id(npc, b"DOFT") {
        Some(doft) => println!("  own DOFT -> 0x{doft:08x}"),
        None => println!("  own DOFT: NONE"),
    }
    match effective_outfit(by_form_id, npc_id) {
        Some((outfit_id, from)) => {
            let outfit_edid = by_form_id
                .get(&outfit_id)
                .and_then(|r| r.edid())
                .unwrap_or_else(|| "?".to_string());
            println!("  effective DOFT -> 0x{outfit_id:08x} ({outfit_edid}) from 0x{from:08x}");
            if let Some(outfit) = by_form_id.get(&outfit_id) {
                for inam in outfit.fields(b"INAM") {
                    for id in id_list(&inam.data) {
                        match by_form_id.get(&id) {
                            Some(rec) => {
                                let e = rec.edid().unwrap_or_else(|| "?".to_string());
                                println!("    INAM -> 0x{id:08x} {} ({e})", rec.type_str());
                                if rec.type_str() == "LVLI" {
                                    for lvlo in rec.fields(b"LVLO") {
                                        if lvlo.data.len() < 10 {
                                            continue;
                                        }
                                        let level =
                                            i16::from_le_bytes(lvlo.data[0..2].try_into().unwrap());
                                        let item =
                                            u32::from_le_bytes(lvlo.data[4..8].try_into().unwrap());
                                        let desc = match by_form_id.get(&item) {
                                            Some(r) => format!(
                                                "{} ({})",
                                                r.type_str(),
                                                r.edid().unwrap_or_else(|| "?".to_string())
                                            ),
                                            None => "NOT FOUND".to_string(),
                                        };
                                        println!(
                                            "      LVLO level={level} item=0x{item:08x} {desc}"
                                        );
                                    }
                                }
                                if rec.type_str() == "ARMO" {
                                    let race = race_id.unwrap_or(0x0000_0019);
                                    match armor_model(by_form_id, id, race, female) {
                                        Some(m) => {
                                            println!("      model: {m}");
                                        }
                                        None => {
                                            println!(
                                                "      model: NONE (no race-matched ARMA model)"
                                            );
                                        }
                                    }
                                }
                            }
                            None => println!("    INAM -> 0x{id:08x} NOT FOUND"),
                        }
                    }
                }
            }
        }
        None => println!("  effective DOFT: NONE (no outfit anywhere in the chain)"),
    }
    let cur = current_chain_piece_count(by_form_id, npc);
    let fixed = fixed_chain_piece_count(by_form_id, npc);
    println!("  outfit pieces: current chain={cur} fixed chain={fixed}");
    // FaceGen existence.
    let fg = format!("meshes/actors/character/facegendata/facegeom/skyrim.esm/{npc_id:08x}.nif");
    println!("  FaceGen {fg} present={}", facegen_present.contains(&fg));
}

fn sweep(by_form_id: &HashMap<u32, &Record>) {
    let npcs: Vec<&Record> = by_form_id
        .values()
        .filter(|r| r.type_str() == "NPC_")
        .copied()
        .collect();
    println!("NPC_ records: {}", npcs.len());
    let with_tplt = npcs
        .iter()
        .filter(|n| field_id(n, b"TPLT").is_some())
        .count();
    let with_own_doft = npcs
        .iter()
        .filter(|n| field_id(n, b"DOFT").is_some())
        .count();
    let with_effective_doft = npcs
        .iter()
        .filter(|n| effective_outfit(by_form_id, n.form_id).is_some())
        .count();
    println!("  with TPLT: {with_tplt}");
    println!("  with own DOFT: {with_own_doft}");
    println!("  with effective DOFT (own or via template): {with_effective_doft}");
    println!(
        "  DOFT only via template: {}",
        with_effective_doft - with_own_doft
    );
    // INAM entry types across all OTFTs.
    let mut inam_armo = 0usize;
    let mut inam_lvli = 0usize;
    let mut inam_other = 0usize;
    let mut inam_missing = 0usize;
    for rec in by_form_id.values() {
        if rec.type_str() != "OTFT" {
            continue;
        }
        for inam in rec.fields(b"INAM") {
            for id in id_list(&inam.data) {
                match by_form_id.get(&id) {
                    Some(r) if r.type_str() == "ARMO" => inam_armo += 1,
                    Some(r) if r.type_str() == "LVLI" => inam_lvli += 1,
                    Some(_) => inam_other += 1,
                    None => inam_missing += 1,
                }
            }
        }
    }
    println!(
        "  OTFT INAM entries: ARMO={inam_armo} LVLI={inam_lvli} other={inam_other} missing={inam_missing}"
    );
    // Piece counts: current vs fixed chain, over all NPCs.
    let mut cur_zero = 0usize;
    let mut fixed_zero = 0usize;
    let mut improved = 0usize;
    let mut improved_list: Vec<(u32, String, usize, usize)> = Vec::new();
    for npc in &npcs {
        let cur = current_chain_piece_count(by_form_id, npc);
        let fixed = fixed_chain_piece_count(by_form_id, npc);
        if cur == 0 {
            cur_zero += 1;
        }
        if fixed == 0 {
            fixed_zero += 1;
        }
        if fixed > cur {
            improved += 1;
            if improved_list.len() < 25 {
                improved_list.push((
                    npc.form_id,
                    npc.edid().unwrap_or_else(|| "?".to_string()),
                    cur,
                    fixed,
                ));
            }
        }
    }
    println!("  NPCs with 0 outfit pieces, current chain: {cur_zero}");
    println!("  NPCs with 0 outfit pieces, fixed chain: {fixed_zero}");
    println!("  NPCs gaining pieces from the fix: {improved}");
    for (id, edid, cur, fixed) in improved_list {
        println!("    0x{id:08x} {edid}: {cur} -> {fixed}");
    }
}

/// The inn door's XTEL arrival (copied from chicken_probe so this
/// probe stands alone).
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
        return Some(SpawnPoint {
            position: [f(1), f(2), f(3)],
            yaw: f(6),
        });
    }
    None
}
