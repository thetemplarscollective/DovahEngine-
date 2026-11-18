//! SPEL (spell) + MGEF (magic effect) + SHOU (shout) parsing
//! for the character menu's Magic tab (UI milestone 2).
//!
//! Every fact here is proven from Dylan's real `Skyrim.esm` by
//! the `m2_data_probe` / `m2_probe2` examples in dovah-app (kept
//! as the durable research record). Counts below are the real
//! file's counts:
//!
//! * SPEL records: **827**. SPIT is 36 bytes everywhere = 9 x
//!   u32/f32. Layout proven on Flames (0x12fcd), Healing
//!   (0x12fcc), Firebolt (0x12fd0), Battle Cry (0xe40c3),
//!   Resist Frost (0xaa020), Combat Heal Rate (0x1031d3):
//!     - @0 base cost (u32): Flames 14, Firebolt 41, Healing 12.
//!     - @4 flags (u32): 0 in all samples but Combat Heal Rate
//!       (131072).
//!     - @8 type (u32): 0 = Spell, 2 = Power (Battle Cry),
//!       4 = Ability (Resist Frost, Combat Heal Rate). Type 3
//!       (Lesser Power) is the documented value, NOT proven by
//!       probe — no lesser power was sampled; it groups with
//!       Powers in the menu either way.
//!     - @12 charge time (f32): Firebolt 0.5, concentration
//!       spells 0.0.
//!     - @16 cast type (u32): 0 = Constant (abilities),
//!       1 = Fire-and-Forget (Firebolt, Battle Cry),
//!       2 = Concentration (Flames, Healing).
//!     - @20 range (u32): 0 = Self (Healing), 2 = Aimed
//!       (Flames, Firebolt).
//!     - @24 perk (form ID): 0 in all samples.
//!     - @28 unknown (u32): 0 in all samples — unresolved,
//!       unused.
//!     - @32 skill-gate perk (form ID): Flames ->
//!       DestructionNovice00 (0xf2ca8), Firebolt ->
//!       DestructionApprentice25 (0xc44bf), Healing ->
//!       RestorationNovice00 (0xf2caa). Powers/abilities carry 0.
//! * Effects: EFID (MGEF form ID) + EFIT (12 bytes) pairs,
//!   positional. Only the EFID list is read here (the menu
//!   shows effect names via the MGEF FULL).
//! * Names: FULL -> `.STRINGS`.
//!
//! ## School of a spell
//!
//! **MGEF DATA @12 (u32) = the magic-skill ActorValue index.**
//! Proven: FireDamageConcAimed @12 = 20 (Destruction),
//! RestoreHealthConcSelf @12 = 22 (Restoration),
//! FireDamageFFAimed @12 = 20. The skill enum (see
//! `skills::skill_index`): Alteration 18, Conjuration 19,
//! Destruction 20, Illusion 21, Restoration 22. A spell's school
//! is its FIRST effect's MGEF skill index. Independent
//! confirmation: SPIT @32 (the skill-gate perk) agrees on all
//! three sampled spells.
//!
//! ## The player's starting spells
//!
//! Player NPC_ 0x00000007: SPLO Flames, Healing, Combat Heal
//! Rate. NordRace SPLO: Resist Frost, Battle Cry. The menu shows
//! what the player actually knows — spells from the player
//! record + the race record, never a hardcoded demo list.
//!
//! ## Shouts
//!
//! 75 SHOU records. SNAM is 12 bytes: word form ID (u32), spell
//! form ID (u32), recovery time f32 (5.0 in the samples). The
//! player knows no shouts at start — the Shouts category is
//! honestly empty until the shout system exists.

use crate::Record;

/// What kind of SPEL this is (SPIT @8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpellKind {
    /// A castable spell (type 0).
    Spell,
    /// A once-a-day power (type 2, e.g. Battle Cry).
    Power,
    /// A lesser power (type 3 — documented value, unproven by
    /// probe; groups with Powers in the menu).
    LesserPower,
    /// A constant ability (type 4, e.g. Resist Frost).
    Ability,
    /// Anything else (disease, poison, voice, enchantment...).
    /// The menu does not list these.
    Other(u32),
}

impl SpellKind {
    pub fn from_type(value: u32) -> Self {
        match value {
            0 => SpellKind::Spell,
            2 => SpellKind::Power,
            3 => SpellKind::LesserPower,
            4 => SpellKind::Ability,
            other => SpellKind::Other(other),
        }
    }
}

/// A magic school (MGEF skill index 18-22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum School {
    Alteration,
    Conjuration,
    Destruction,
    Illusion,
    Restoration,
}

/// The magic school for an ActorValue skill index, or `None`
/// when the index is not a school (non-magic skills, or -1).
pub fn school_for_skill_index(index: i32) -> Option<School> {
    match index {
        18 => Some(School::Alteration),
        19 => Some(School::Conjuration),
        20 => Some(School::Destruction),
        21 => Some(School::Illusion),
        22 => Some(School::Restoration),
        _ => None,
    }
}

/// A parsed SPEL record (spell, power, or ability).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellDef {
    pub form_id: u32,
    pub edid: Option<String>,
    /// Spell name string ID (`FULL`) — resolves in `.STRINGS`.
    pub full_id: Option<u32>,
    /// Base magicka cost (SPIT @0).
    pub cost: u32,
    pub kind: SpellKind,
    /// Cast type (SPIT @16): 0 = Constant, 1 = Fire-and-Forget,
    /// 2 = Concentration.
    pub cast_type: u32,
    /// Range (SPIT @20): 0 = Self, 2 = Aimed (others unseen in
    /// the samples).
    pub range: u32,
    /// Effect form IDs (EFID fields, in file order).
    pub effects: Vec<u32>,
    /// The skill-gate perk (SPIT @32), e.g. DestructionNovice00.
    /// `None` when the field is 0 (powers, abilities).
    pub skill_gate_perk: Option<u32>,
}

/// Parse a SPEL record. `None` when the record is not a SPEL.
/// A SPEL without SPIT still parses (cost 0, kind Spell) — the
/// caller decides what to show; all 827 real SPELs carry SPIT.
pub fn parse_spell(record: &Record) -> Option<SpellDef> {
    if &record.record_type != b"SPEL" {
        return None;
    }
    let mut cost = 0;
    let mut kind = SpellKind::Spell;
    let mut cast_type = 0;
    let mut range = 0;
    let mut skill_gate_perk = None;
    if let Some(spit) = record.field(b"SPIT") {
        if spit.data.len() == 36 {
            let w = |i: usize| u32::from_le_bytes(spit.data[i * 4..i * 4 + 4].try_into().unwrap());
            cost = w(0);
            kind = SpellKind::from_type(w(2));
            cast_type = w(4);
            range = w(5);
            if w(8) != 0 {
                skill_gate_perk = Some(w(8));
            }
        }
    }
    let effects = record
        .fields(b"EFID")
        .filter_map(|f| u32_4(&f.data))
        .collect();
    Some(SpellDef {
        form_id: record.form_id,
        edid: record.edid(),
        full_id: record.field(b"FULL").and_then(|f| u32_4(&f.data)),
        cost,
        kind,
        cast_type,
        range,
        effects,
        skill_gate_perk,
    })
}

/// The magic-skill ActorValue index of an MGEF record (DATA @12
/// as i32; -1 = no skill). `None` when the record is not an
/// MGEF or has no 152-byte DATA field.
pub fn mgef_skill(record: &Record) -> Option<i32> {
    if &record.record_type != b"MGEF" {
        return None;
    }
    let data = record.field(b"DATA")?;
    if data.data.len() < 16 {
        return None;
    }
    Some(i32::from_le_bytes(data.data[12..16].try_into().unwrap()))
}

/// All SPLO (spell) form IDs on a record (NPC_ or RACE), in
/// file order. The player's known spells are the player
/// record's SPLO plus the race record's SPLO.
pub fn spells_of(record: &Record) -> Vec<u32> {
    record
        .fields(b"SPLO")
        .filter_map(|f| u32_4(&f.data))
        .collect()
}

/// One shout word: the WORD form ID, the spell it unlocks, and
/// the shout recovery time in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct ShoutWord {
    pub word_form_id: u32,
    pub spell_form_id: u32,
    pub recovery_seconds: f32,
}

/// A parsed SHOU record (shout).
#[derive(Debug, Clone, PartialEq)]
pub struct ShoutDef {
    pub form_id: u32,
    pub edid: Option<String>,
    /// Shout name string ID (`FULL`) — resolves in `.STRINGS`.
    pub full_id: Option<u32>,
    /// Words, in file order (usually 3).
    pub words: Vec<ShoutWord>,
}

/// Parse a SHOU record. `None` when the record is not a SHOU.
pub fn parse_shout(record: &Record) -> Option<ShoutDef> {
    if &record.record_type != b"SHOU" {
        return None;
    }
    let mut words = Vec::new();
    for snam in record.fields(b"SNAM") {
        if snam.data.len() != 12 {
            continue;
        }
        words.push(ShoutWord {
            word_form_id: u32::from_le_bytes(snam.data[0..4].try_into().unwrap()),
            spell_form_id: u32::from_le_bytes(snam.data[4..8].try_into().unwrap()),
            recovery_seconds: f32::from_le_bytes(snam.data[8..12].try_into().unwrap()),
        });
    }
    Some(ShoutDef {
        form_id: record.form_id,
        edid: record.edid(),
        full_id: record.field(b"FULL").and_then(|f| u32_4(&f.data)),
        words,
    })
}

fn u32_4(data: &[u8]) -> Option<u32> {
    if data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(data.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Field;

    fn field(code: &[u8; 4], data: &[u8]) -> Field {
        Field {
            field_type: *code,
            data: data.to_vec(),
        }
    }

    fn record(record_type: &[u8; 4], form_id: u32, fields: Vec<Field>) -> Record {
        Record {
            record_type: *record_type,
            flags: 0,
            form_id,
            version: 44,
            fields,
        }
    }

    /// SPIT bytes for the given (cost, kind, cast type, range,
    /// skill-gate perk) — the other words zero, charge time 0.
    fn spit(cost: u32, kind: u32, cast_type: u32, range: u32, perk: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(36);
        for w in [cost, 0, kind, 0, cast_type, range, 0, 0, perk] {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out
    }

    #[test]
    fn parse_spell_flames_shape() {
        // Flames 0x00012fcd: cost 14, type Spell, concentration,
        // aimed, gate DestructionNovice00, two effects.
        let rec = record(
            b"SPEL",
            0x0001_2fcd,
            vec![
                field(b"EDID", b"Flames\0"),
                field(b"FULL", &7195u32.to_le_bytes()),
                field(b"SPIT", &spit(14, 0, 2, 2, 0x000f_2ca8)),
                field(b"EFID", &0x0001_3ca9u32.to_le_bytes()),
                field(b"EFIT", &[0; 12]),
                field(b"EFID", &0x000f_392fu32.to_le_bytes()),
                field(b"EFIT", &[0; 12]),
            ],
        );
        let spell = parse_spell(&rec).expect("a SPEL parses");
        assert_eq!(spell.form_id, 0x0001_2fcd);
        assert_eq!(spell.edid.as_deref(), Some("Flames"));
        assert_eq!(spell.full_id, Some(7195));
        assert_eq!(spell.cost, 14);
        assert_eq!(spell.kind, SpellKind::Spell);
        assert_eq!(spell.cast_type, 2);
        assert_eq!(spell.range, 2);
        assert_eq!(spell.skill_gate_perk, Some(0x000f_2ca8));
        assert_eq!(spell.effects, vec![0x0001_3ca9, 0x000f_392f]);
    }

    #[test]
    fn parse_spell_power_and_ability() {
        // Battle Cry 0x000e40c3: type 2 (Power), cost 0,
        // fire-and-forget, self, no gate perk.
        let cry = record(
            b"SPEL",
            0x000e_40c3,
            vec![
                field(b"EDID", b"BattleCry\0"),
                field(b"SPIT", &spit(0, 2, 1, 0, 0)),
            ],
        );
        let spell = parse_spell(&cry).expect("a Power parses");
        assert_eq!(spell.kind, SpellKind::Power);
        assert_eq!(spell.cost, 0);
        assert_eq!(spell.skill_gate_perk, None);
        // Resist Frost 0x000aa020: type 4 (Ability), constant.
        let frost = record(
            b"SPEL",
            0x000a_a020,
            vec![
                field(b"EDID", b"ResistFrost\0"),
                field(b"SPIT", &spit(0, 4, 0, 0, 0)),
            ],
        );
        let spell = parse_spell(&frost).expect("an Ability parses");
        assert_eq!(spell.kind, SpellKind::Ability);
        assert_eq!(spell.cast_type, 0);
    }

    #[test]
    fn parse_spell_rejects_non_spel() {
        let rec = record(b"MGEF", 0x0001_0001, vec![]);
        assert_eq!(parse_spell(&rec), None);
    }

    #[test]
    fn mgef_skill_reads_data_offset_12() {
        // FireDamageConcAimed shape: DATA 152 bytes, skill 20
        // (Destruction) at @12.
        let mut data = vec![0u8; 152];
        data[12..16].copy_from_slice(&20u32.to_le_bytes());
        let rec = record(
            b"MGEF",
            0x0001_3ca9,
            vec![
                field(b"EDID", b"FireDamageConcAimed\0"),
                field(b"DATA", &data),
            ],
        );
        assert_eq!(mgef_skill(&rec), Some(20));
        assert_eq!(school_for_skill_index(20), Some(School::Destruction));
        // No skill: -1.
        let mut data_none = vec![0u8; 152];
        data_none[12..16].copy_from_slice(&(-1i32).to_le_bytes());
        let rec_none = record(b"MGEF", 0x0001_0002, vec![field(b"DATA", &data_none)]);
        assert_eq!(mgef_skill(&rec_none), Some(-1));
        assert_eq!(school_for_skill_index(-1), None);
        // Not an MGEF.
        let rec_spel = record(b"SPEL", 0x0001_0003, vec![]);
        assert_eq!(mgef_skill(&rec_spel), None);
    }

    #[test]
    fn school_for_skill_index_maps_the_five_schools() {
        assert_eq!(school_for_skill_index(18), Some(School::Alteration));
        assert_eq!(school_for_skill_index(19), Some(School::Conjuration));
        assert_eq!(school_for_skill_index(21), Some(School::Illusion));
        assert_eq!(school_for_skill_index(22), Some(School::Restoration));
        assert_eq!(school_for_skill_index(6), None); // OneHanded
        assert_eq!(school_for_skill_index(23), None); // Enchanting
    }

    #[test]
    fn spells_of_collects_splo_in_order() {
        // Player 0x00000007 shape: Flames, Healing, Combat Heal
        // Rate.
        let rec = record(
            b"NPC_",
            0x0000_0007,
            vec![
                field(b"SPLO", &0x0001_2fcdu32.to_le_bytes()),
                field(b"SPLO", &0x0001_2fccu32.to_le_bytes()),
                field(b"SPLO", &0x0010_31d3u32.to_le_bytes()),
            ],
        );
        assert_eq!(spells_of(&rec), vec![0x0001_2fcd, 0x0001_2fcc, 0x0010_31d3]);
    }

    #[test]
    fn parse_shout_three_words() {
        // L_DragonFrostIceStormShout shape: 3 SNAMs, recovery 5.0.
        let snam = |word: u32, spell: u32| {
            let mut out = Vec::with_capacity(12);
            out.extend_from_slice(&word.to_le_bytes());
            out.extend_from_slice(&spell.to_le_bytes());
            out.extend_from_slice(&5.0f32.to_le_bytes());
            field(b"SNAM", &out)
        };
        let rec = record(
            b"SHOU",
            0x0010_fe24,
            vec![
                field(b"EDID", b"L_DragonFrostIceStormShout\0"),
                field(b"FULL", &9472u32.to_le_bytes()),
                snam(0x0002_52c3, 0x0010_fe20),
                snam(0x0002_52c4, 0x0010_fe20),
                snam(0x0002_52c5, 0x0010_fe20),
            ],
        );
        let shout = parse_shout(&rec).expect("a SHOU parses");
        assert_eq!(shout.words.len(), 3);
        assert_eq!(shout.words[0].word_form_id, 0x0002_52c3);
        assert_eq!(shout.words[0].spell_form_id, 0x0010_fe20);
        assert_eq!(shout.words[0].recovery_seconds, 5.0);
        assert_eq!(parse_shout(&record(b"SPEL", 1, vec![])), None);
    }
}
