//! NPC assembly (M12): resolve an NPC_ record to the model paths
//! that draw the person — body parts, outfit armor, and the FaceGen
//! head.
//!
//! The chain, proven field by field against the real Skyrim.esm
//! (M12 probing):
//! - NPC_ `DOFT` (4-byte form ID) -> OTFT (outfit).
//! - OTFT `INAM` is ONE field holding an ARRAY of form IDs
//!   (4/8/12/16/20/24 bytes = 1-6 items), not one field per item.
//! - ARMO `MODL` is a 4-byte FORM ID pointing to an ARMA record —
//!   NOT a path (the "garbage" bytes are the form ID). An ARMO can
//!   list MULTIPLE MODL fields = multiple ARMAs (race variants).
//!   All 4,298 MODL fields in Skyrim.esm are 4 bytes and resolve
//!   to ARMA records. ARMO `MOD2`/`MOD4` are GROUND models — the
//!   dropped item, not the worn mesh.
//! - ARMA: `MOD2` = male biped model, `MOD3` = female biped model,
//!   `MOD4`/`MOD5` = 1st person (skipped). `RNAM` = race; pick the
//!   ARMA whose RNAM matches the NPC's race, else DefaultRace
//!   (0x00000019), else the first. 172 ARMAs lack MOD3 (male-only
//!   items) — a missing model is skipped, not an error.
//! - NPC_ `RNAM` -> RACE; NPC_ `ACBS` flags bit 0 = female.
//! - NPC_ `CNTO` = inventory (carried, NOT worn) — the outfit is
//!   the appearance, so CNTO is ignored.
//! - Body (bare skin): the `NakedTorso` / `NakedHands` /
//!   `NakedFeet` ARMAs, found by EDID + race (falling back to
//!   DefaultRace). The body is always rendered under armor —
//!   authentic, the armor meshes cover it.
//! - Head: the FaceGen NIF is a COMPLETE head — do NOT load the
//!   separate HDPT head-part files (those are the generic
//!   templates; the FaceGen file is the per-NPC generated version
//!   with the right hair/eyes/brows/beard baked in). Path:
//!   `Actors\Character\FaceGenData\FaceGeom\Skyrim.esm\<formid
//!   8-hex lowercase>.nif`. Only mod index 0 (Skyrim.esm) is
//!   supported — the app loads Skyrim.esm only, so every NPC it
//!   sees is index 0; DLC NPCs need the load order (later).
//! - OTFT entries can be LVLI (leveled lists), not just ARMO:
//!   the guard outfit is ONE LVLI entry. An LVLI resolves by its
//!   `LVLF` flags byte: bit 2 (0x04) "Use All" gives EVERY
//!   eligible entry (a set — the guard list is cuirass + boots +
//!   shield + a nested helmet list); without it the list gives
//!   ONE deterministic pick (the per-slot variety lists —
//!   LItemGutterSaintBoots holds 9 boot variants, one is worn).
//!   `LVLD` is Chance None (a percent), honored on pick-one
//!   lists, ignored on Use-All lists (the flag means "ignore
//!   chance"). Entries are level-gated at player level 1 (no
//!   progression yet). Proven by `lvli_raw_probe` +
//!   `lvld_probe` (v0.4.6 phone feedback, the naked guard).
//! - NPC_ `TPLT` (template): when the NPC has no own DOFT, the
//!   outfit is inherited from the first template in the chain
//!   that has one. `TPTF` (template flags) never occurs in
//!   Skyrim.esm (0/3651 template NPCs), so there is nothing to
//!   gate the walk on — proven by `facegen_signal_probe` Part B.
//!
//! Limitations (documented, not bugs): no RACE body-part list
//! lookup (the Naked* ARMAs are the standard human body —
//! mannequin/Astrid variants are NPC-specific overrides the
//! outfit chain doesn't reach); no 1st-person models; no
//! hair/eyes from HDPT (FaceGen covers it). 1,187 of 3,735
//! humanoid NPCs (32%) have NO FaceGen file — the game assembles
//! their heads at runtime from head parts, which this engine
//! does not do yet, so they render headless (the Riverwood
//! guards 01/03 are the visible case). No record-level signal
//! predicts the missing file (proven by `facegen_signal_probe`:
//! race, TPLT, NAM9, TINI all fail) — the fallback needs the
//! asset index at resolve time, which `resolve_npc` does not
//! have (cell.rs loads from the plugin only). Follow-up, not
//! this milestone.

use dovah_esm::{Field, Record};
use std::collections::HashMap;

/// The DefaultRace form ID (0x00000019): the fallback race for
/// ARMA and body-part lookups.
pub const DEFAULT_RACE_FORM_ID: u32 = 0x0000_0019;

/// The model paths that draw one NPC, relative to `meshes/`
/// (e.g. `Actors\Character\Character Assets\MaleBody_1.NIF`).
/// Order: body parts (torso, hands, feet), outfit parts, head.
#[derive(Debug, Clone)]
pub struct NpcModels {
    pub parts: Vec<String>,
    /// The NPC's sex (ACBS flags bit 0). Unused by the renderer —
    /// the parts already encode it — but the animation and dialogue
    /// milestones will need it, so the resolution reports it.
    #[allow(dead_code)]
    pub female: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BodyKind {
    Torso,
    Hands,
    Feet,
}

impl BodyKind {
    /// The exact base EDID for this kind.
    fn base_edid(self) -> &'static str {
        match self {
            BodyKind::Torso => "NakedTorso",
            BodyKind::Hands => "NakedHands",
            BodyKind::Feet => "NakedFeet",
        }
    }

    /// The kind of an ARMA EDID, if it is a body-part ARMA.
    /// `NakedBody*` (mannequin/Astrid) is deliberately NOT a body
    /// kind — those are NPC-specific overrides, not the standard
    /// body.
    fn from_edid(edid: &str) -> Option<BodyKind> {
        if edid.starts_with("NakedTorso") {
            Some(BodyKind::Torso)
        } else if edid.starts_with("NakedHands") {
            Some(BodyKind::Hands)
        } else if edid.starts_with("NakedFeet") {
            Some(BodyKind::Feet)
        } else {
            None
        }
    }
}

/// Index of the body-part ARMAs, built once per load:
/// (race, kind) -> (male model, female model). Exact-EDID matches
/// win over prefix matches (`NakedTorsoAfflicted` must not shadow
/// `NakedTorso`).
pub struct NpcIndex {
    naked: HashMap<(u32, BodyKind), (Option<String>, Option<String>)>,
}

impl NpcIndex {
    pub fn build(by_form_id: &HashMap<u32, &Record>) -> Self {
        let mut naked: HashMap<(u32, BodyKind), (Option<String>, Option<String>)> = HashMap::new();
        // Two passes: exact base EDIDs first, then prefix matches
        // only for keys the exact pass didn't fill.
        for exact_only in [true, false] {
            for record in by_form_id.values() {
                if record.type_str() != "ARMA" {
                    continue;
                }
                let Some(edid) = record.edid() else {
                    continue;
                };
                let Some(kind) = BodyKind::from_edid(&edid) else {
                    continue;
                };
                let is_exact = edid == kind.base_edid();
                if exact_only != is_exact {
                    continue;
                }
                let Some(race) = form_id_from_field(record, b"RNAM") else {
                    continue;
                };
                let key = (race, kind);
                if naked.contains_key(&key) {
                    continue;
                }
                naked.insert(key, (arma_model(record, false), arma_model(record, true)));
            }
        }
        NpcIndex { naked }
    }

    /// The body-part model for a race and kind, falling back to
    /// DefaultRace. `None` when neither has the part.
    fn body(&self, race: u32, kind: BodyKind, female: bool) -> Option<String> {
        for r in [race, DEFAULT_RACE_FORM_ID] {
            if let Some((male, female_model)) = self.naked.get(&(r, kind)) {
                let model = if female { female_model } else { male };
                if let Some(model) = model {
                    return Some(model.clone());
                }
            }
        }
        None
    }
}

/// The 4-byte form ID of a field, or `None` when the field is
/// missing or not 4 bytes.
fn form_id_from_field(record: &Record, field_type: &[u8; 4]) -> Option<u32> {
    let field = record.field(field_type)?;
    if field.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(field.data[0..4].try_into().unwrap()))
}

/// All form IDs in a field that holds an array of them (OTFT
/// `INAM`). Trailing partial bytes are ignored.
fn form_id_list(field: &Field) -> Vec<u32> {
    field
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

/// An ARMA's worn biped model: `MOD2` (male) or `MOD3` (female).
/// `None` when the model field is missing or empty.
fn arma_model(arma: &Record, female: bool) -> Option<String> {
    let code = if female { b"MOD3" } else { b"MOD2" };
    let model = arma.field(code)?.as_str();
    if model.is_empty() {
        None
    } else {
        Some(model)
    }
}

/// Pick the ARMA for a race: exact race match, else DefaultRace,
/// else the first. `None` when the list is empty.
fn pick_arma<'a>(armas: &[&'a Record], race: u32) -> Option<&'a Record> {
    if armas.is_empty() {
        return None;
    }
    for wanted in [race, DEFAULT_RACE_FORM_ID] {
        if let Some(arma) = armas
            .iter()
            .find(|a| form_id_from_field(a, b"RNAM") == Some(wanted))
        {
            return Some(arma);
        }
    }
    Some(armas[0])
}

/// The FaceGen head path for an NPC form ID, or `None` when the
/// NPC is not from mod index 0 (Skyrim.esm) — DLC NPCs need the
/// load order, which the app doesn't have yet.
fn facegen_path(npc_form_id: u32) -> Option<String> {
    if npc_form_id >> 24 != 0 {
        return None;
    }
    Some(format!(
        "Actors\\Character\\FaceGenData\\FaceGeom\\Skyrim.esm\\{npc_form_id:08x}.nif"
    ))
}

/// The bare body parts (torso, hands, feet) for a race and sex —
/// the skin rendered under armor. M17: the player model reuses
/// this (the NPC pipeline's body, not a second copy of it).
pub fn body_parts(index: &NpcIndex, race: u32, female: bool) -> Vec<String> {
    let mut parts = Vec::new();
    for kind in [BodyKind::Torso, BodyKind::Hands, BodyKind::Feet] {
        if let Some(model) = index.body(race, kind, female) {
            parts.push(model);
        }
    }
    parts
}

/// The worn biped model of one ARMO for a race and sex: ARMO
/// `MODL` form IDs -> race-matched ARMA -> `MOD2`/`MOD3`. `None`
/// when the form ID is not an ARMO, no ARMA matches, or the ARMA
/// has no model for this sex (male-only items). M17: the player
/// model resolves each equipped armor piece through this — the
/// same chain the NPC outfit resolution uses, factored out so
/// there is one code path.
pub fn armor_model(
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
    let arma = pick_arma(&armas, race)?;
    arma_model(arma, female)
}

/// LVLI `LVLF` flags byte, bit 2 (0x04) = "Use All": the list
/// gives EVERY eligible entry, not one pick. The two outfit-set
/// lists prove it — OutfitListGuardWhiterun (0x000d33c7) and
/// ArmorIronSetAll (0x00013739) both carry LVLF=0x04, while the
/// per-slot variety lists (LItemGutterSaint*) carry 0x03
/// (calc-all-levels + calc-each) and must pick one. `LVLD` is
/// NOT the flags — it is Chance None, a percent (0/25/50/75/100
/// across the plugin). Proven by `lvli_raw_probe`.
pub const LVLI_FLAG_USE_ALL: u8 = 0x04;

/// One LVLO entry: (required level, item form ID). LVLO is 12
/// bytes — level i16 @0, unused i16 @2, form ID u32 @4, count
/// i16 @8, unused i16 @10. Count is ignored for outfits: a worn
/// piece is worn once.
fn lvli_entries(lvli: &Record) -> Vec<(i16, u32)> {
    lvli.fields(b"LVLO")
        .filter_map(|f| {
            if f.data.len() < 8 {
                return None;
            }
            let level = i16::from_le_bytes(f.data[0..2].try_into().unwrap());
            let item = u32::from_le_bytes(f.data[4..8].try_into().unwrap());
            Some((level, item))
        })
        .collect()
}

/// Resolve an outfit's leveled list to item form IDs. Two rules,
/// by the LVLF flags (see `LVLI_FLAG_USE_ALL`):
///
/// - Use All (0x04): EVERY entry at or below player level 1
///   (an outfit set — the guard list is cuirass + boots + shield
///   plus a nested helmet list). Nested lists recurse and apply
///   their OWN flag.
/// - Otherwise: ONE deterministic pick among the eligible
///   entries (the M17 container rule, seeded by the NPC instead
///   of a container — the same NPC always wears the same pick,
///   different NPCs vary, like the game's random roll). Chance
///   None (LVLD) is honored here: the roll may yield nothing.
///   Use-All lists ignore chance (the flag means "ignore
///   chance" — and both real set lists have LVLD=0 anyway).
///
/// Depth cap 4 (M17). Player level 1 until progression exists;
/// at level 1 the calc-all-levels flag (0x01) changes nothing —
/// only level-1 entries are eligible either way.
fn resolve_outfit_lvli(
    by_form_id: &HashMap<u32, &Record>,
    lvli_id: u32,
    npc_form_id: u32,
    depth: u32,
) -> Vec<u32> {
    if depth > 4 {
        return Vec::new();
    }
    let Some(lvli) = by_form_id.get(&lvli_id) else {
        return Vec::new();
    };
    if lvli.type_str() != "LVLI" {
        return Vec::new();
    }
    let eligible: Vec<u32> = lvli_entries(lvli)
        .into_iter()
        .filter(|(level, _)| *level <= 1)
        .map(|(_, item)| item)
        .collect();
    if eligible.is_empty() {
        return Vec::new();
    }
    let use_all = lvli
        .field(b"LVLF")
        .and_then(|f| f.data.first())
        .map(|b| b & LVLI_FLAG_USE_ALL != 0)
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
    // Pick-one: the deterministic hash, mirroring M17's
    // `resolve_lvli` (Knuth multiplicative hash; the seed is the
    // NPC so different NPCs sharing a list vary).
    let hash = lvli_id
        .wrapping_mul(2_654_435_761)
        .wrapping_add(npc_form_id.wrapping_mul(40_503))
        .wrapping_add(depth.wrapping_mul(97));
    let chance_none = lvli
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

/// The NPC's effective default outfit: its own DOFT, else the
/// first DOFT found walking the TPLT template chain (depth cap
/// 8 — also the cycle guard). TPTF gating is NOT applied: the
/// field never occurs in Skyrim.esm (0/3651 template NPCs), so
/// there is nothing to gate on. Only 23 NPCs get their outfit
/// this way, most of them creatures the resolver rejects anyway
/// (Draugr, Falmer, Dwarven automatons).
fn effective_outfit_id(by_form_id: &HashMap<u32, &Record>, npc: &Record) -> Option<u32> {
    let mut current = Some(npc);
    let mut depth = 0;
    while let Some(record) = current {
        if let Some(doft) = form_id_from_field(record, b"DOFT") {
            return Some(doft);
        }
        if depth >= 8 {
            return None;
        }
        depth += 1;
        current = form_id_from_field(record, b"TPLT").and_then(|id| by_form_id.get(&id).copied());
    }
    None
}

/// RACE `DATA` flags: a u32 at offset 32 of the 164-byte DATA
/// field. Bit 1 (0x2) is the FaceGen-head flag — the race uses
/// the humanoid face/body pipeline. Proven by the
/// `race_flag_sweep` probe over every RACE record in
/// Skyrim.esm: set on all ten playable races and their child /
/// vampire variants, on Dremora, Elder, and DefaultRace; clear
/// on every animal and monster race (Chicken, Cow, Dog, Horse,
/// Mudcrab, Wolf, Dragon, Draugr, Falmer, Giant, Hagraven,
/// Spriggan, Troll, ...). Skyrim has no CREA record type —
/// creatures ARE NPC_ records, and this flag is the
/// creature/person line.
pub const RACE_FLAG_FACEGEN_HEAD: u32 = 0x2;

/// Whether a RACE record is a humanoid race (FaceGen-head flag
/// set). A missing or malformed DATA field reads as NOT
/// humanoid — a broken race record must never dress an animal
/// in a human body.
pub fn race_has_facegen(race: &Record) -> bool {
    race.field(b"DATA")
        .and_then(|f| {
            if f.data.len() < 36 {
                return None;
            }
            Some(u32::from_le_bytes(f.data[32..36].try_into().unwrap()))
        })
        .map(|flags| flags & RACE_FLAG_FACEGEN_HEAD != 0)
        .unwrap_or(false)
}

/// Whether an NPC_ base is a creature: its race record exists
/// and lacks the FaceGen-head flag. A base with no race field,
/// or a race record that is not in the plugin, reads as NOT a
/// creature — the humanoid default (the pre-existing behaviour
/// for the inn's NPCs, whose fixture races are absent in
/// tests). A missing base, or a base that is not NPC_ at all,
/// reads as a creature: it is certainly not a person.
pub fn npc_is_creature(by_form_id: &HashMap<u32, &Record>, npc_form_id: u32) -> bool {
    let Some(npc) = by_form_id.get(&npc_form_id) else {
        return true;
    };
    if npc.type_str() != "NPC_" {
        return true;
    }
    let Some(race_id) = form_id_from_field(npc, b"RNAM") else {
        return false;
    };
    let Some(race) = by_form_id.get(&race_id) else {
        return false;
    };
    !race_has_facegen(race)
}

/// Resolve an NPC_ record to the model paths that draw the person:
/// body parts, outfit armor, and the FaceGen head. `None` when the
/// form ID is not an NPC_ record in this plugin — and `None` for
/// creatures (chicken, cow, horse, dragon...): their race has no
/// FaceGen head, so the human body pipeline does not apply, and
/// dressing them in it renders a naked man where the game has an
/// animal (the chicken dialogue bug — Dylan walked up to the
/// chicken's human body and talked to "Chicken"). Creatures render
/// as the red capsule until the creature-model milestone.
pub fn resolve_npc(
    by_form_id: &HashMap<u32, &Record>,
    index: &NpcIndex,
    npc_form_id: u32,
) -> Option<NpcModels> {
    let npc = by_form_id.get(&npc_form_id)?;
    if npc.type_str() != "NPC_" {
        return None;
    }
    if npc_is_creature(by_form_id, npc_form_id) {
        return None;
    }
    let female = npc
        .field(b"ACBS")
        .and_then(|f| f.data.first().map(|b| b & 1 == 1))
        .unwrap_or(false);
    let race = form_id_from_field(npc, b"RNAM").unwrap_or(DEFAULT_RACE_FORM_ID);

    // Body first: always rendered under armor.
    let mut parts = body_parts(index, race, female);
    // Outfit: effective DOFT (own, else via the template chain)
    // -> OTFT -> INAM array -> ARMO or LVLI -> race-matched
    // ARMA -> MOD2/MOD3 (via `armor_model`, shared with M17).
    // LVLI entries resolve by their LVLF flag (Use All = the
    // whole set, else one deterministic pick) — the M12 chain
    // passed every INAM id straight to `armor_model`, which
    // returns None for a non-ARMO, so any outfit whose entries
    // are lists (the Riverwood guards: ONE LVLI entry) rendered
    // with no outfit at all — the v0.4.6 naked guard.
    if let Some(outfit_id) = effective_outfit_id(by_form_id, npc) {
        if let Some(outfit) = by_form_id.get(&outfit_id) {
            for inam in outfit.fields(b"INAM") {
                for entry_id in form_id_list(inam) {
                    let Some(entry) = by_form_id.get(&entry_id) else {
                        continue;
                    };
                    match entry.type_str().as_str() {
                        "ARMO" => {
                            if let Some(model) = armor_model(by_form_id, entry_id, race, female) {
                                parts.push(model);
                            }
                        }
                        "LVLI" => {
                            for armo_id in resolve_outfit_lvli(by_form_id, entry_id, npc_form_id, 0)
                            {
                                if let Some(model) = armor_model(by_form_id, armo_id, race, female)
                                {
                                    parts.push(model);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    // Head: the FaceGen NIF is the complete head.
    if let Some(head) = facegen_path(npc_form_id) {
        parts.push(head);
    }
    Some(NpcModels { parts, female })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dovah_esm::Field;

    fn field(code: &[u8; 4], data: Vec<u8>) -> Field {
        Field {
            field_type: *code,
            data,
        }
    }

    fn str_field(code: &[u8; 4], value: &str) -> Field {
        let mut data = value.as_bytes().to_vec();
        data.push(0);
        field(code, data)
    }

    fn id_field(code: &[u8; 4], id: u32) -> Field {
        field(code, id.to_le_bytes().to_vec())
    }

    fn record(type_code: &[u8; 4], form_id: u32, fields: Vec<Field>) -> Record {
        Record {
            record_type: *type_code,
            flags: 0,
            form_id,
            version: 44,
            fields,
        }
    }

    fn arma(form_id: u32, edid: &str, race: u32, male: &str, female: &str) -> Record {
        let mut fields = vec![str_field(b"EDID", edid), id_field(b"RNAM", race)];
        if !male.is_empty() {
            fields.push(str_field(b"MOD2", male));
        }
        if !female.is_empty() {
            fields.push(str_field(b"MOD3", female));
        }
        record(b"ARMA", form_id, fields)
    }

    /// A minimal plugin index: the three DefaultRace body ARMAs
    /// plus one outfit chain (female robe + male-only boots).
    fn test_index() -> (HashMap<u32, Record>, NpcIndex) {
        let records = vec![
            arma(
                0x0000_0d67,
                "NakedTorso",
                DEFAULT_RACE_FORM_ID,
                "MaleBody_1.NIF",
                "FemaleBody_1.nif",
            ),
            arma(
                0x0000_0d6c,
                "NakedHands",
                DEFAULT_RACE_FORM_ID,
                "MaleHands_1.nif",
                "FemaleHands_1.nif",
            ),
            arma(
                0x0000_0d6e,
                "NakedFeet",
                DEFAULT_RACE_FORM_ID,
                "MaleFeet_1.nif",
                "FemaleFeet_1.nif",
            ),
            // Afflicted variant must not shadow the exact EDID.
            arma(
                0x0000_0e00,
                "NakedTorsoAfflicted",
                DEFAULT_RACE_FORM_ID,
                "AfflictedBody.nif",
                "AfflictedBodyF.nif",
            ),
            // Robe: Breton + Nord ARMAs; the Breton one wins for a
            // Breton NPC.
            arma(
                0x0000_1001,
                "RobeBretonAA",
                0x0001_3741,
                "robe_m.nif",
                "robe_f.nif",
            ),
            arma(
                0x0000_1002,
                "RobeNordAA",
                0x0001_3746,
                "robe_nord_m.nif",
                "robe_nord_f.nif",
            ),
            // Boots: male-only (no MOD3).
            arma(
                0x0000_1003,
                "BootsAA",
                DEFAULT_RACE_FORM_ID,
                "boots_m.nif",
                "",
            ),
            record(
                b"ARMO",
                0x0000_2001,
                vec![
                    str_field(b"EDID", "ClothesRobe"),
                    id_field(b"MODL", 0x0000_1001),
                    id_field(b"MODL", 0x0000_1002),
                ],
            ),
            record(
                b"ARMO",
                0x0000_2002,
                vec![
                    str_field(b"EDID", "ClothesBoots"),
                    id_field(b"MODL", 0x0000_1003),
                ],
            ),
            record(
                b"OTFT",
                0x0000_3001,
                vec![field(
                    b"INAM",
                    [0x0000_2001u32.to_le_bytes(), 0x0000_2002u32.to_le_bytes()].concat(),
                )],
            ),
            record(
                b"NPC_",
                0x0001_3478,
                vec![
                    str_field(b"EDID", "TestFemaleBreton"),
                    id_field(b"RNAM", 0x0001_3741),
                    field(b"ACBS", vec![0x01, 0x00, 0x00, 0x00]),
                    id_field(b"DOFT", 0x0000_3001),
                ],
            ),
            record(
                b"NPC_",
                0x0001_3479,
                vec![
                    str_field(b"EDID", "TestMaleNordNoOutfit"),
                    id_field(b"RNAM", 0x0001_3746),
                    field(b"ACBS", vec![0x00, 0x00, 0x00, 0x00]),
                ],
            ),
        ];
        let by_form_id: HashMap<u32, Record> =
            records.into_iter().map(|r| (r.form_id, r)).collect();
        let refs: HashMap<u32, &Record> = by_form_id.iter().map(|(id, r)| (*id, r)).collect();
        let index = NpcIndex::build(&refs);
        (by_form_id, index)
    }

    #[test]
    fn female_breton_resolves_full_chain() {
        let (by_form_id, index) = test_index();
        let refs: HashMap<u32, &Record> = by_form_id.iter().map(|(id, r)| (*id, r)).collect();
        let models = resolve_npc(&refs, &index, 0x0001_3478).unwrap();
        assert!(models.female);
        // Body (exact EDID wins over Afflicted) + robe (Breton
        // ARMA, female model) + head. Boots are male-only, so a
        // female NPC skips them.
        assert_eq!(
            models.parts,
            vec![
                "FemaleBody_1.nif",
                "FemaleHands_1.nif",
                "FemaleFeet_1.nif",
                "robe_f.nif",
                "Actors\\Character\\FaceGenData\\FaceGeom\\Skyrim.esm\\00013478.nif",
            ]
        );
    }

    #[test]
    fn male_nord_without_outfit_is_body_plus_head() {
        let (by_form_id, index) = test_index();
        let refs: HashMap<u32, &Record> = by_form_id.iter().map(|(id, r)| (*id, r)).collect();
        let models = resolve_npc(&refs, &index, 0x0001_3479).unwrap();
        assert!(!models.female);
        assert_eq!(
            models.parts,
            vec![
                "MaleBody_1.NIF",
                "MaleHands_1.nif",
                "MaleFeet_1.nif",
                "Actors\\Character\\FaceGenData\\FaceGeom\\Skyrim.esm\\00013479.nif",
            ]
        );
    }

    #[test]
    fn race_fallback_picks_default_arma() {
        let (by_form_id, _index) = test_index();
        let refs: HashMap<u32, &Record> = by_form_id.iter().map(|(id, r)| (*id, r)).collect();
        // Boots ARMA is DefaultRace only: a Nord NPC still gets it
        // (male model).
        let boots = refs[&0x0000_2002];
        let armas: Vec<&Record> = boots
            .fields(b"MODL")
            .filter_map(|f| {
                let id = u32::from_le_bytes(f.data[0..4].try_into().unwrap());
                refs.get(&id).copied()
            })
            .collect();
        let picked = pick_arma(&armas, 0x0001_3746).unwrap();
        assert_eq!(picked.form_id, 0x0000_1003);
        assert_eq!(arma_model(picked, false).unwrap(), "boots_m.nif");
        // ...and a female NPC gets no model from a male-only ARMA.
        assert!(arma_model(picked, true).is_none());
    }

    #[test]
    fn non_npc_and_missing_form_ids_return_none() {
        let (by_form_id, index) = test_index();
        let refs: HashMap<u32, &Record> = by_form_id.iter().map(|(id, r)| (*id, r)).collect();
        assert!(resolve_npc(&refs, &index, 0x0000_2001).is_none()); // ARMO, not NPC_
        assert!(resolve_npc(&refs, &index, 0x0000_9999).is_none()); // missing
    }

    /// A synthetic RACE record: a 164-byte DATA field with the
    /// given flags u32 at offset 32 (the real layout).
    fn race_record(form_id: u32, edid: &str, flags: u32) -> Record {
        let mut data = vec![0u8; 164];
        data[32..36].copy_from_slice(&flags.to_le_bytes());
        record(
            b"RACE",
            form_id,
            vec![str_field(b"EDID", edid), field(b"DATA", data)],
        )
    }

    #[test]
    fn creature_races_get_no_human_body() {
        // The real flag values from race_flag_sweep:
        // ChickenRace 0x00100908 (no FaceGen head), NordRace
        // 0x50a08943 (FaceGen head set).
        let (mut by_form_id, index) = test_index();
        by_form_id.insert(
            0x000a_919d,
            race_record(0x000a_919d, "ChickenRace", 0x0010_0908),
        );
        by_form_id.insert(
            0x0001_3746,
            race_record(0x0001_3746, "NordRace", 0x50a0_8943),
        );
        by_form_id.insert(
            0x000a_91a0,
            record(
                b"NPC_",
                0x000a_91a0,
                vec![
                    str_field(b"EDID", "EncChicken"),
                    id_field(b"RNAM", 0x000a_919d),
                    field(b"ACBS", vec![0x00, 0x00, 0x00, 0x00]),
                ],
            ),
        );
        by_form_id.insert(
            0x0001_3480,
            record(
                b"NPC_",
                0x0001_3480,
                vec![
                    str_field(b"EDID", "TestMaleNord"),
                    id_field(b"RNAM", 0x0001_3746),
                    field(b"ACBS", vec![0x00, 0x00, 0x00, 0x00]),
                ],
            ),
        );
        // An NPC_ whose race record is absent from the plugin:
        // the humanoid default (the pre-existing behaviour).
        by_form_id.insert(
            0x0001_3500,
            record(
                b"NPC_",
                0x0001_3500,
                vec![
                    str_field(b"EDID", "TestNoRaceRecord"),
                    id_field(b"RNAM", 0x0000_8888),
                    field(b"ACBS", vec![0x00, 0x00, 0x00, 0x00]),
                ],
            ),
        );
        let refs: HashMap<u32, &Record> = by_form_id.iter().map(|(id, r)| (*id, r)).collect();

        // The flag itself, on the real values.
        assert!(!race_has_facegen(refs[&0x000a_919d]));
        assert!(race_has_facegen(refs[&0x0001_3746]));
        // The creature predicate.
        assert!(npc_is_creature(&refs, 0x000a_91a0));
        assert!(!npc_is_creature(&refs, 0x0001_3480));
        assert!(!npc_is_creature(&refs, 0x0001_3500));
        assert!(npc_is_creature(&refs, 0x0000_2001)); // ARMO: not a person
        assert!(npc_is_creature(&refs, 0x0000_9999)); // missing: not a person
                                                      // The rendering consequence: the chicken gets no human
                                                      // body (the scene falls back to the red capsule); the
                                                      // Nord and the race-less NPC resolve as before.
        assert!(resolve_npc(&refs, &index, 0x000a_91a0).is_none());
        assert!(resolve_npc(&refs, &index, 0x0001_3480).is_some());
        assert!(resolve_npc(&refs, &index, 0x0001_3500).is_some());
    }

    #[test]
    fn facegen_path_only_for_mod_index_zero() {
        assert_eq!(
            facegen_path(0x0001_3478).unwrap(),
            "Actors\\Character\\FaceGenData\\FaceGeom\\Skyrim.esm\\00013478.nif"
        );
        // Mod index 1 (e.g. Update.esm) has no path yet.
        assert!(facegen_path(0x0101_3478).is_none());
    }

    /// An LVLI record: LVLD (chance none), LVLF (flags), LLCT
    /// (count), one 12-byte LVLO per entry (level i16 @0, form
    /// ID u32 @4, count i16 @8) — the real layout.
    fn lvli(
        form_id: u32,
        edid: &str,
        chance_none: u8,
        flags: u8,
        entries: &[(i16, u32)],
    ) -> Record {
        let mut fields = vec![
            str_field(b"EDID", edid),
            field(b"LVLD", vec![chance_none]),
            field(b"LVLF", vec![flags]),
            field(b"LLCT", vec![entries.len() as u8]),
        ];
        for (level, item) in entries {
            let mut data = vec![0u8; 12];
            data[0..2].copy_from_slice(&level.to_le_bytes());
            data[4..8].copy_from_slice(&item.to_le_bytes());
            data[8..10].copy_from_slice(&1i16.to_le_bytes());
            fields.push(field(b"LVLO", data));
        }
        record(b"LVLI", form_id, fields)
    }

    fn male_nord_npc(form_id: u32, edid: &str, extra: Vec<Field>) -> Record {
        let mut fields = vec![
            str_field(b"EDID", edid),
            id_field(b"RNAM", 0x0001_3746),
            field(b"ACBS", vec![0x00, 0x00, 0x00, 0x00]),
        ];
        fields.extend(extra);
        record(b"NPC_", form_id, fields)
    }

    /// `test_index` plus the v0.4.6 guard-mirror fixtures: a
    /// Use-All guard list (robe + boots + nested 1-entry helmet
    /// list), a pick-one variety list, a chance-none-100 list,
    /// and template NPCs. The guard NPC reuses the real
    /// GuardRiverwoodImperial01 form ID (0x000274ab).
    fn lvli_index() -> (HashMap<u32, Record>, NpcIndex) {
        let (mut by_form_id, index) = test_index();
        by_form_id.insert(
            0x0000_1004,
            arma(
                0x0000_1004,
                "HelmetAA",
                DEFAULT_RACE_FORM_ID,
                "helmet_m.nif",
                "helmet_f.nif",
            ),
        );
        by_form_id.insert(
            0x0000_2003,
            record(
                b"ARMO",
                0x0000_2003,
                vec![
                    str_field(b"EDID", "ClothesHelmet"),
                    id_field(b"MODL", 0x0000_1004),
                ],
            ),
        );
        // The guard list, mirroring OutfitListGuardWhiterun
        // (LVLF 0x04, one nested helmet list).
        by_form_id.insert(
            0x0000_4001,
            lvli(
                0x0000_4001,
                "GuardList",
                0,
                LVLI_FLAG_USE_ALL,
                &[(1, 0x0000_2001), (1, 0x0000_2002), (1, 0x0000_4002)],
            ),
        );
        by_form_id.insert(
            0x0000_4002,
            lvli(0x0000_4002, "HelmetList", 0, 0, &[(1, 0x0000_2003)]),
        );
        // A per-slot variety list: three pieces, one worn.
        by_form_id.insert(
            0x0000_4003,
            lvli(
                0x0000_4003,
                "VarietyList",
                0,
                0,
                &[(1, 0x0000_2001), (1, 0x0000_2002), (1, 0x0000_2003)],
            ),
        );
        // Chance None 100: the list never yields.
        by_form_id.insert(
            0x0000_4004,
            lvli(0x0000_4004, "CursedList", 100, 0, &[(1, 0x0000_2001)]),
        );
        let otft = |form_id: u32, entries: &[u32]| {
            let data: Vec<u8> = entries.iter().flat_map(|id| id.to_le_bytes()).collect();
            record(b"OTFT", form_id, vec![field(b"INAM", data)])
        };
        by_form_id.insert(0x0000_3002, otft(0x0000_3002, &[0x0000_4001]));
        by_form_id.insert(0x0000_3003, otft(0x0000_3003, &[0x0000_4003]));
        by_form_id.insert(0x0000_3004, otft(0x0000_3004, &[0x0000_4004]));
        by_form_id.insert(0x0000_3005, otft(0x0000_3005, &[0x0000_2003]));
        by_form_id.insert(
            0x0002_74ab,
            male_nord_npc(
                0x0002_74ab,
                "GuardRiverwoodImperial01",
                vec![id_field(b"DOFT", 0x0000_3002)],
            ),
        );
        by_form_id.insert(
            0x0002_74ac,
            male_nord_npc(
                0x0002_74ac,
                "VarietyWearer",
                vec![id_field(b"DOFT", 0x0000_3003)],
            ),
        );
        by_form_id.insert(
            0x0002_74ad,
            male_nord_npc(
                0x0002_74ad,
                "CursedWearer",
                vec![id_field(b"DOFT", 0x0000_3004)],
            ),
        );
        by_form_id.insert(
            0x0000_3100,
            male_nord_npc(
                0x0000_3100,
                "TemplateWithOutfit",
                vec![id_field(b"DOFT", 0x0000_3001)],
            ),
        );
        by_form_id.insert(
            0x0000_3101,
            male_nord_npc(
                0x0000_3101,
                "TemplatedNoOutfit",
                vec![id_field(b"TPLT", 0x0000_3100)],
            ),
        );
        by_form_id.insert(
            0x0000_3102,
            male_nord_npc(
                0x0000_3102,
                "OwnOutfitBeatsTemplate",
                vec![
                    id_field(b"DOFT", 0x0000_3005),
                    id_field(b"TPLT", 0x0000_3100),
                ],
            ),
        );
        (by_form_id, index)
    }

    #[test]
    fn guard_outfit_resolves_use_all_list() {
        // The v0.4.6 naked guard, mirrored: the outfit's only
        // INAM entry is a Use-All LVLI (robe + boots + nested
        // helmet list). The old chain gave 0 outfit pieces.
        let (by_form_id, index) = lvli_index();
        let refs: HashMap<u32, &Record> = by_form_id.iter().map(|(id, r)| (*id, r)).collect();
        let models = resolve_npc(&refs, &index, 0x0002_74ab).unwrap();
        assert_eq!(
            models.parts,
            vec![
                "MaleBody_1.NIF",
                "MaleHands_1.nif",
                "MaleFeet_1.nif",
                "robe_nord_m.nif",
                "boots_m.nif",
                "helmet_m.nif",
                "Actors\\Character\\FaceGenData\\FaceGeom\\Skyrim.esm\\000274ab.nif",
            ]
        );
    }

    #[test]
    fn pick_one_list_wears_exactly_one_piece() {
        // A variety list (no Use All) is ONE pick, not the whole
        // list — the take-all rule would stack all three pieces.
        // Which one is the deterministic hash's pick; the test
        // pins the count and the membership, not the pick.
        let (by_form_id, index) = lvli_index();
        let refs: HashMap<u32, &Record> = by_form_id.iter().map(|(id, r)| (*id, r)).collect();
        let models = resolve_npc(&refs, &index, 0x0002_74ac).unwrap();
        assert_eq!(models.parts.len(), 5); // body x3 + 1 outfit + head
        assert!(matches!(
            models.parts[3].as_str(),
            "robe_nord_m.nif" | "boots_m.nif" | "helmet_m.nif"
        ));
        // And the pick is stable: the same NPC resolves the same
        // way every time.
        let again = resolve_npc(&refs, &index, 0x0002_74ac).unwrap();
        assert_eq!(models.parts, again.parts);
    }

    #[test]
    fn chance_none_hundred_yields_no_outfit_piece() {
        let (by_form_id, index) = lvli_index();
        let refs: HashMap<u32, &Record> = by_form_id.iter().map(|(id, r)| (*id, r)).collect();
        let models = resolve_npc(&refs, &index, 0x0002_74ad).unwrap();
        assert_eq!(
            models.parts,
            vec![
                "MaleBody_1.NIF",
                "MaleHands_1.nif",
                "MaleFeet_1.nif",
                "Actors\\Character\\FaceGenData\\FaceGeom\\Skyrim.esm\\000274ad.nif",
            ]
        );
    }

    #[test]
    fn outfit_inherited_from_template() {
        let (by_form_id, index) = lvli_index();
        let refs: HashMap<u32, &Record> = by_form_id.iter().map(|(id, r)| (*id, r)).collect();
        let models = resolve_npc(&refs, &index, 0x0000_3101).unwrap();
        assert_eq!(
            models.parts,
            vec![
                "MaleBody_1.NIF",
                "MaleHands_1.nif",
                "MaleFeet_1.nif",
                "robe_nord_m.nif",
                "boots_m.nif",
                "Actors\\Character\\FaceGenData\\FaceGeom\\Skyrim.esm\\00003101.nif",
            ]
        );
    }

    #[test]
    fn own_doft_beats_template_doft() {
        let (by_form_id, index) = lvli_index();
        let refs: HashMap<u32, &Record> = by_form_id.iter().map(|(id, r)| (*id, r)).collect();
        let models = resolve_npc(&refs, &index, 0x0000_3102).unwrap();
        assert_eq!(
            models.parts,
            vec![
                "MaleBody_1.NIF",
                "MaleHands_1.nif",
                "MaleFeet_1.nif",
                "helmet_m.nif",
                "Actors\\Character\\FaceGenData\\FaceGeom\\Skyrim.esm\\00003102.nif",
            ]
        );
    }
}
