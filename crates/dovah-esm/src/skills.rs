//! AVIF (actor value / skill) + RACE skill-boost parsing for
//! the character menu's Skills tab (UI milestone 2).
//!
//! Every fact here is proven from Dylan's real `Skyrim.esm` by
//! the `m2_data_probe` / `m2_probe2` examples in dovah-app (kept
//! as the durable research record).
//!
//! ## The 18 skills
//!
//! AVIF records 0x0000044c-0x0000045d, consecutive, in
//! ActorValue skill order: OneHanded, TwoHanded, Marksman,
//! Block, Smithing, HeavyArmor, LightArmor, Pickpocket,
//! Lockpicking, Sneak, Alchemy, Speechcraft, Alteration,
//! Conjuration, Destruction, Illusion (EDID `AVMysticism` — the
//! record keeps its Oblivion name), Restoration, Enchanting.
//! `form_id = 0x446 + skill_index` (OneHanded 6 -> 0x44c,
//! Enchanting 23 -> 0x45d). FULL resolves in `.STRINGS`
//! ("One-handed", "Archery", "Speech", "Light Armour", ...);
//! DESC (a string ID) resolves in `.DLSTRINGS` to the real
//! skill descriptions (all 6 sampled resolve).
//!
//! Spelling note: the game's own English strings use British
//! spelling ("Light Armour", "Heavy Armour"). The menu shows
//! the data as-is.
//!
//! ## Racial skill boosts
//!
//! RACE DATA (164 bytes) starts with 7 x (skill i8, boost i8)
//! pairs at @0-13, terminated by (-1, 0). Proven on two races:
//! NordRace (TwoHanded +10, OneHanded/Block/Smithing/Speech/
//! LightArmor +5 — NO Heavy Armor boost in the data) and
//! BretonRace (Conjuration +10, Illusion/Restoration/Speech/
//! Alchemy/Alteration +5). RACE DATA also holds male/female
//! height f32 @16/20 and the flags u32 @32.
//!
//! Starting skill level = [`BASE_SKILL_LEVEL`] (15, the game's
//! base rule) + the racial boost.
//!
//! ## Guardian grouping
//!
//! The Skills tab groups the 18 skills into the game's three
//! guardian-stone groups — Warrior, Thief, Mage, 6 skills each
//! (see [`GUARDIAN_GROUPS`]). This grouping is game knowledge
//! (the guardian stones), NOT data in the AVIF records; the
//! order inside each group is the ActorValue enum order (= AVIF
//! form-ID order).

use crate::Record;

/// The game's base skill level: every skill starts at 15, plus
/// the racial boost.
pub const BASE_SKILL_LEVEL: u8 = 15;

/// First skill AVIF form ID (OneHanded, skill index 6).
pub const FIRST_SKILL_FORM_ID: u32 = 0x0000_044c;

/// Last skill AVIF form ID (Enchanting, skill index 23).
pub const LAST_SKILL_FORM_ID: u32 = 0x0000_045d;

/// The number of skills (18).
pub const SKILL_COUNT: usize = 18;

/// The ActorValue skill index for a form ID, or `None` when it
/// is not one of the 18 skill AVIFs. `form_id = 0x446 + index`.
pub fn skill_index(form_id: u32) -> Option<u8> {
    if (FIRST_SKILL_FORM_ID..=LAST_SKILL_FORM_ID).contains(&form_id) {
        Some((form_id - 0x446) as u8)
    } else {
        None
    }
}

/// The AVIF form ID for an ActorValue skill index (6-23), or
/// `None` outside that range.
pub fn skill_form_id(index: u8) -> Option<u32> {
    if (6..=23).contains(&index) {
        Some(0x446 + index as u32)
    } else {
        None
    }
}

/// The three guardian-stone groups, each with its 6 skill
/// indices in ActorValue order: Warrior (OneHanded, TwoHanded,
/// Marksman, Block, Smithing, HeavyArmor), Thief (LightArmor,
/// Pickpocket, Lockpicking, Sneak, Alchemy, Speechcraft), Mage
/// (Alteration, Conjuration, Destruction, Illusion, Restoration,
/// Enchanting).
pub const GUARDIAN_GROUPS: [(&str, [u8; 6]); 3] = [
    ("Warrior", [6, 7, 8, 9, 10, 11]),
    ("Thief", [12, 13, 14, 15, 16, 17]),
    ("Mage", [18, 19, 20, 21, 22, 23]),
];

/// A parsed skill AVIF record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDef {
    /// ActorValue skill index (6-23).
    pub index: u8,
    pub form_id: u32,
    pub edid: Option<String>,
    /// Skill name string ID (`FULL`) — resolves in `.STRINGS`.
    pub full_id: Option<u32>,
    /// Skill description string ID (`DESC`) — resolves in
    /// `.DLSTRINGS`.
    pub desc_id: Option<u32>,
}

/// Parse a skill AVIF record. `None` when the record is not an
/// AVIF or is not one of the 18 skills (the other ~130 AVIFs —
/// PowerMods, SkillAdvances, conditions — are not skills).
pub fn parse_skill(record: &Record) -> Option<SkillDef> {
    if &record.record_type != b"AVIF" {
        return None;
    }
    let index = skill_index(record.form_id)?;
    Some(SkillDef {
        index,
        form_id: record.form_id,
        edid: record.edid(),
        full_id: record.field(b"FULL").and_then(|f| u32_4(&f.data)),
        desc_id: record.field(b"DESC").and_then(|f| u32_4(&f.data)),
    })
}

/// The racial skill boosts on a RACE record: (skill index,
/// boost) pairs from DATA @0-13, stopping at the (-1, 0)
/// terminator (or the 7-pair limit). Empty when the record is
/// not a RACE or has no DATA field.
pub fn race_skill_boosts(record: &Record) -> Vec<(u8, i8)> {
    if &record.record_type != b"RACE" {
        return Vec::new();
    }
    let Some(data) = record.field(b"DATA") else {
        return Vec::new();
    };
    let mut boosts = Vec::new();
    for i in 0..7 {
        let Some(pair) = data.data.get(i * 2..i * 2 + 2) else {
            break;
        };
        let skill = pair[0] as i8;
        let boost = pair[1] as i8;
        if skill < 0 {
            break;
        }
        boosts.push((skill as u8, boost));
    }
    boosts
}

/// A skill's starting level for a race: 15 + the racial boost
/// (0 when the race does not boost this skill).
pub fn starting_skill_level(boosts: &[(u8, i8)], skill_index: u8) -> u8 {
    let boost = boosts
        .iter()
        .find(|(s, _)| *s == skill_index)
        .map(|(_, b)| *b)
        .unwrap_or(0);
    (BASE_SKILL_LEVEL as i16 + boost as i16).max(0) as u8
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

    #[test]
    fn skill_index_maps_the_18_avifs() {
        assert_eq!(skill_index(0x0000_044c), Some(6)); // OneHanded
        assert_eq!(skill_index(0x0000_045a), Some(20)); // Destruction
        assert_eq!(skill_index(0x0000_045d), Some(23)); // Enchanting
        assert_eq!(skill_index(0x0000_044b), None);
        assert_eq!(skill_index(0x0000_045e), None);
        assert_eq!(skill_index(0x0000_0607), None); // AVOneHandedMod
        assert_eq!(skill_form_id(6), Some(0x0000_044c));
        assert_eq!(skill_form_id(23), Some(0x0000_045d));
        assert_eq!(skill_form_id(5), None);
        assert_eq!(skill_form_id(24), None);
    }

    #[test]
    fn parse_skill_one_handed_shape() {
        // AVOneHanded 0x0000044c: FULL 4897-ish name, DESC text.
        let rec = record(
            b"AVIF",
            0x0000_044c,
            vec![
                field(b"EDID", b"AVOneHanded\0"),
                field(b"FULL", &4897u32.to_le_bytes()),
                field(b"DESC", &4897u32.to_le_bytes()),
            ],
        );
        let skill = parse_skill(&rec).expect("a skill AVIF parses");
        assert_eq!(skill.index, 6);
        assert_eq!(skill.full_id, Some(4897));
        assert_eq!(skill.desc_id, Some(4897));
        // A non-skill AVIF does not parse as a skill.
        let mod_rec = record(
            b"AVIF",
            0x0000_0607,
            vec![field(b"EDID", b"AVOneHandedMod\0")],
        );
        assert_eq!(parse_skill(&mod_rec), None);
        assert_eq!(parse_skill(&record(b"SPEL", 1, vec![])), None);
    }

    #[test]
    fn race_skill_boosts_nord_shape() {
        // NordRace DATA head: (7,+10) (6,+5) (9,+5) (10,+5)
        // (17,+5) (12,+5) (-1,0), then heights.
        let mut data = vec![0u8; 164];
        let pairs: [(u8, u8); 7] = [
            (7, 10),
            (6, 5),
            (9, 5),
            (10, 5),
            (17, 5),
            (12, 5),
            (0xff, 0),
        ];
        for (i, (s, b)) in pairs.iter().enumerate() {
            data[i * 2] = *s;
            data[i * 2 + 1] = *b;
        }
        let rec = record(
            b"RACE",
            0x0001_3746,
            vec![field(b"EDID", b"NordRace\0"), field(b"DATA", &data)],
        );
        let boosts = race_skill_boosts(&rec);
        assert_eq!(
            boosts,
            vec![(7, 10), (6, 5), (9, 5), (10, 5), (17, 5), (12, 5)]
        );
        // Starting levels: Two-Handed 25, One-Handed 20,
        // Destruction (unboosted) 15.
        assert_eq!(starting_skill_level(&boosts, 7), 25);
        assert_eq!(starting_skill_level(&boosts, 6), 20);
        assert_eq!(starting_skill_level(&boosts, 20), 15);
        // Not a RACE.
        assert!(race_skill_boosts(&record(b"NPC_", 1, vec![])).is_empty());
    }

    #[test]
    fn guardian_groups_cover_all_18_skills_once() {
        let mut seen = [false; 24];
        for (_, skills) in GUARDIAN_GROUPS {
            for s in skills {
                assert!((6..=23).contains(&s));
                assert!(!seen[s as usize], "skill {s} in two groups");
                seen[s as usize] = true;
            }
        }
        assert!(seen[6..=23].iter().all(|s| *s));
    }
}
