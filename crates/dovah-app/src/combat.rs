//! Melee combat: fists + weapons.
//!
//! The minimal loop: attack input (touch ATTACK R/L buttons,
//! desktop mouse Left) -> instant hit on the nearest living
//! NPC in range and in front of the player -> damage from the
//! equipped weapon's WEAP record (or fists) -> NPC health from
//! the vanilla formula -> death as a rigid fall.
//!
//! What this milestone is NOT: there is no .hkx animation
//! system yet and no first-person viewmodel (no first-person
//! hands or weapons exist), so there is nothing visible to
//! swing — the attack lands INSTANTLY on press, gated by the
//! weapon's swing cooldown (1 / WEAP DNAM speed). The visible
//! swing, the viewmodel, and the animation system are a
//! follow-up milestone. NPCs do not fight back (no AI/aggro
//! system yet) and the player cannot die.
//!
//! Vanilla vs implemented, death: vanilla plays a death
//! animation (the actor crumples to the floor over ~1 s, then
//! lies as a corpse the player can loot). Implemented: the
//! actor's parts rotate rigidly 90 degrees about the feet on
//! the killing blow — a static fall, not a ragdoll, not an
//! animation — and the corpse stays where it fell. Looting a
//! corpse is a follow-up (it needs the container UI pointed at
//! a dead actor's inventory).
//!
//! Health (UESP CLAS/Health pages; the formula matches 935/935
//! vanilla auto-calc NPCs per the tes4skyrim analysis):
//! ```text
//! Health = racial base (RACE DATA f32 @36; 50 for playable
//!          races, 5 for the chicken)
//!        + NPC offset (NPC_ ACBS i16 @20)
//!        + 5 * (level - 1)                              [auto-calc only]
//!        + 10 * (level - 1) * wH / (wH + wM + wS)       [auto-calc only]
//! ```
//! where wH/wM/wS are the CLAS DATA bytes 32-34. Level is the
//! ACBS i16 @8, except when the PC Level Mult flag (0x80) is
//! set — then it is the player level times (field / 1000),
//! clamped to [calc min, calc max] (ACBS u16 @10/@12).
//! Delphine's field is 900 (0.9x), clamped to 10 at player
//! level 1; Orgnar and Embry are fixed level 4. DNAM.Health is
//! only a cache and is NOT read (its values are unrelated to
//! the formula — Orgnar's cache reads 6424).
//!
//! Weapon stats (proven by the melee_probe against known
//! vanilla values): WEAP DATA is 10 bytes (value i32 @0,
//! weight f32 @4, damage u16 @8); WEAP DNAM is 100 bytes
//! (animation type u32 @0, speed f32 @4, reach f32 @8).
//! Iron Sword: damage 7, speed 1.0, reach 1.0. Fists: damage
//! 4 (vanilla unarmed base), speed 1.0, reach 1.0.
//!
//! Reach: the DNAM reach stat is a multiplier on a 1.5 m base
//! (an engine calibration — vanilla resolves hits with hitbox
//! sweeps, and the records carry no metres). A sword reaches
//! 1.5 m, a dagger 1.05 m, a greatsword 1.95 m. The arc is
//! +/-45 degrees (dot > 0.7071), also an engine choice.

use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions};
use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;

use crate::cell::CellData;
use crate::coords;
use crate::dialogue::DialogueOpen;
use crate::inventory::{Inventory, InventoryOpen, ItemCatalog, ItemKind};
use crate::pause::PauseOpen;
use crate::player::EYE_HEIGHT;
use crate::save::WorldChanges;
use crate::scene::Player;
use crate::touch::TouchInputState;
use crate::SharedPlugin;

/// Unarmed base damage (vanilla unarmed damage is 4).
pub const UNARMED_DAMAGE: u16 = 4;
/// Unarmed swing speed (1 swing/second — the sword's speed).
pub const UNARMED_SPEED: f32 = 1.0;
/// Unarmed reach stat (same multiplier as a sword).
pub const UNARMED_REACH: f32 = 1.0;
/// Base melee reach in metres; the weapon's DNAM reach stat
/// multiplies it (see the module docs).
pub const BASE_MELEE_REACH_M: f32 = 1.5;
/// Cosine of the melee arc half-angle: +/-45 degrees
/// (1/sqrt(2)).
pub const MELEE_ARC_COS: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// Vertical tolerance for a melee target (same as USE).
pub const MELEE_HEIGHT_M: f32 = 2.0;
/// The death fall: pitch 90 degrees about the actor's local X
/// axis. The head (+Y) goes to local +Z (behind the actor —
/// actors face local -Z), so the body falls BACKWARD, face up.
pub const FALL_PITCH: f32 = FRAC_PI_2;
/// Player level for PC-Level-Mult NPCs (no XP system yet, so
/// the player is always level 1).
pub const PLAYER_LEVEL: i32 = 1;
/// ACBS flag bits (NPC_ ACBS u32 @0).
pub const ACBS_AUTO_CALC_STATS: u32 = 0x10;
pub const ACBS_PC_LEVEL_MULT: u32 = 0x80;
/// `--melee-test` teleport distance from the target (inside
/// fist reach, far enough to frame the body).
pub const MELEE_TEST_DISTANCE_M: f32 = 1.2;

/// Marker for one rendered part of an NPC: every sub-mesh
/// entity of an actor's models (plus the red-capsule fallback)
/// carries it, keyed by the actor's ref form ID. The death
/// system finds the parts to fall through this tag.
#[derive(Component, Debug, Clone, Copy)]
pub struct NpcPart {
    pub ref_form_id: u32,
}

/// One fightable actor in the loaded scene.
#[derive(Debug, Clone)]
pub struct Combatant {
    pub base_form_id: u32,
    /// Display name for the log (the base EDID — "Orgnar").
    pub name: String,
    /// Feet position in Bevy metres.
    pub position: Vec3,
    /// Vanilla level (ACBS, with PC Level Mult applied).
    pub level: i32,
    pub max_health: i32,
    pub health: i32,
    pub dead: bool,
}

/// All fightable actors in the loaded scene, by ref form ID.
/// Rebuilt on every scene entry (see `build_combatants`).
#[derive(Debug, Default, Resource)]
pub struct Combatants {
    pub npcs: HashMap<u32, Combatant>,
}

/// Time left before the next swing is allowed (seconds).
/// Set to the weapon's cooldown on every swing, ticked down
/// by `melee_attack`.
#[derive(Debug, Default, Resource)]
pub struct SwingCooldown {
    pub remaining: f32,
}

/// The `--melee-test N` seed (desktop proof hook): teleport
/// the player in front of the seeded NPC and queue N scripted
/// attacks through the real attack system. The NPC is Orgnar
/// (NPC_ 0x00013479) when present, else the nearest
/// combatant to the player.
#[derive(Debug, Resource)]
pub struct MeleeTestSeed {
    pub attacks: u32,
    pub npc_base: Option<u32>,
}

/// Scripted attacks still to fire (one per swing — the queue
/// decrements only when a swing actually happens, so presses
/// during the cooldown are not lost).
#[derive(Debug, Resource)]
pub struct MeleeTestQueue {
    pub attacks_remaining: u32,
}

/// Melee combat plugin: resources + the attack system. Added
/// by both the windowed and screenshot paths (the screenshot
/// path needs the attack system for `--melee-test`).
pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Combatants>()
            .init_resource::<SwingCooldown>()
            .add_systems(
                Update,
                melee_attack.run_if(in_state(crate::AppState::InGame)),
            );
    }
}

// ----------------------------------------------------------------
// Little-endian field readers (same shape as inventory.rs's).
// ----------------------------------------------------------------

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    data.get(offset..offset + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn i16_at(data: &[u8], offset: usize) -> Option<i16> {
    u16_at(data, offset).map(|v| v as i16)
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    data.get(offset..offset + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn f32_at(data: &[u8], offset: usize) -> Option<f32> {
    u32_at(data, offset).map(f32::from_bits)
}

// ----------------------------------------------------------------
// Health — pure.
// ----------------------------------------------------------------

/// An NPC's level from its ACBS fields. With PC Level Mult
/// (flag 0x80) the level field is a multiplier times 1000
/// (Delphine: 900 = 0.9x the player level), clamped to
/// [calc_min, calc_max]; otherwise the field IS the level.
pub fn npc_level(
    flags: u32,
    level_field: i16,
    calc_min: u16,
    calc_max: u16,
    player_level: i32,
) -> i32 {
    if flags & ACBS_PC_LEVEL_MULT != 0 {
        let scaled = player_level as f32 * (level_field as f32 / 1000.0);
        (scaled as i32)
            .clamp(calc_min as i32, calc_max as i32)
            .max(1)
    } else {
        (level_field as i32).max(1)
    }
}

/// Max health from the vanilla formula (see the module docs).
/// Non-auto-calc NPCs get no per-level gains (racial + offset
/// only); a zero class-weight sum skips the class share.
pub fn npc_max_health(
    racial_health: f32,
    offset: i16,
    auto_calc: bool,
    level: i32,
    health_weight: u8,
    magicka_weight: u8,
    stamina_weight: u8,
) -> i32 {
    let mut health = racial_health as i32 + offset as i32;
    if auto_calc {
        let sum = health_weight as i32 + magicka_weight as i32 + stamina_weight as i32;
        health += 5 * (level - 1);
        if sum > 0 {
            health += 10 * (level - 1) * health_weight as i32 / sum;
        }
    }
    health.max(1)
}

/// (level, max health) for one NPC_ record, resolved through
/// its CLAS and RACE records. Missing records degrade
/// gracefully: no class = no class share, no race = 50
/// (the playable-race base), no ACBS = level 1, no offset.
pub fn npc_health_from_records(
    npc: &dovah_esm::Record,
    class: Option<&dovah_esm::Record>,
    race: Option<&dovah_esm::Record>,
    player_level: i32,
) -> (i32, i32) {
    let (flags, level_field, calc_min, calc_max, offset) = match npc.field(b"ACBS") {
        Some(f) => (
            u32_at(&f.data, 0).unwrap_or(0),
            i16_at(&f.data, 8).unwrap_or(1),
            u16_at(&f.data, 10).unwrap_or(0),
            u16_at(&f.data, 12).unwrap_or(0),
            i16_at(&f.data, 20).unwrap_or(0),
        ),
        None => (0, 1, 0, 0, 0),
    };
    let level = npc_level(flags, level_field, calc_min, calc_max, player_level);
    let racial = race
        .and_then(|r| r.field(b"DATA"))
        .and_then(|d| f32_at(&d.data, 36))
        .unwrap_or(50.0);
    let (wh, wm, ws) = match class.and_then(|c| c.field(b"DATA")) {
        Some(d) => (
            d.data.get(32).copied().unwrap_or(0),
            d.data.get(33).copied().unwrap_or(0),
            d.data.get(34).copied().unwrap_or(0),
        ),
        None => (0, 0, 0),
    };
    let health = npc_max_health(
        racial,
        offset,
        flags & ACBS_AUTO_CALC_STATS != 0,
        level,
        wh,
        wm,
        ws,
    );
    (level, health)
}

// ----------------------------------------------------------------
// Hit detection + damage — pure.
// ----------------------------------------------------------------

/// Effective melee reach in metres for a weapon reach stat.
pub fn melee_reach_m(reach_stat: f32) -> f32 {
    BASE_MELEE_REACH_M * reach_stat
}

/// Swing cooldown in seconds for a weapon speed stat.
pub fn swing_cooldown_secs(speed: f32) -> f32 {
    1.0 / speed.max(0.1)
}

/// Melee range test: within reach horizontally, within the
/// vertical tolerance of the eye, and inside the +/-45 degree
/// arc in front of the player. Point-blank (the target under
/// the player's feet) always passes the arc test.
pub fn in_melee_range(target: Vec3, feet: Vec3, forward: Vec3, reach_m: f32) -> bool {
    let dx = target.x - feet.x;
    let dz = target.z - feet.z;
    let horizontal = (dx * dx + dz * dz).sqrt();
    if horizontal > reach_m {
        return false;
    }
    let eye_y = feet.y + EYE_HEIGHT;
    if (eye_y - target.y).abs() > MELEE_HEIGHT_M {
        return false;
    }
    if horizontal < 0.01 {
        return true;
    }
    let to_target = Vec3::new(dx / horizontal, 0.0, dz / horizontal);
    let flat = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    flat.dot(to_target) >= MELEE_ARC_COS
}

/// The nearest LIVING combatant in melee range, by ref form
/// ID. Dead NPCs are skipped (corpses are not hit).
pub fn nearest_combatant_in_range(
    combatants: &Combatants,
    feet: Vec3,
    forward: Vec3,
    reach_m: f32,
) -> Option<u32> {
    let mut best: Option<(u32, f32)> = None;
    for (ref_form_id, npc) in &combatants.npcs {
        if npc.dead {
            continue;
        }
        if !in_melee_range(npc.position, feet, forward, reach_m) {
            continue;
        }
        let dx = npc.position.x - feet.x;
        let dz = npc.position.z - feet.z;
        let dist_sq = dx * dx + dz * dz;
        if best.map(|(_, d)| dist_sq < d).unwrap_or(true) {
            best = Some((*ref_form_id, dist_sq));
        }
    }
    best.map(|(ref_form_id, _)| ref_form_id)
}

/// Apply damage to a health pool: (new health, died).
pub fn apply_damage(health: i32, damage: u16) -> (i32, bool) {
    let new = health - damage as i32;
    if new <= 0 {
        (0, true)
    } else {
        (new, false)
    }
}

// ----------------------------------------------------------------
// Scene entry.
// ----------------------------------------------------------------

/// Build the combatant index for the loaded scene (OnEnter
/// InGame, after `build_talkable_npcs`). Health resolves
/// through the NPC_'s CLAS and RACE records in one pass over
/// the plugin (NPC_ records for this cell's actors, plus all
/// CLAS/RACE records — a few hundred). NPCs already in
/// `WorldChanges.dead_npcs` spawn dead.
pub fn build_combatants(
    cell: Option<Res<CellData>>,
    plugin: Option<Res<SharedPlugin>>,
    world: Res<WorldChanges>,
    mut combatants: ResMut<Combatants>,
) {
    combatants.npcs.clear();
    let (Some(cell), Some(plugin)) = (cell, plugin) else {
        return;
    };
    let wanted: std::collections::HashSet<u32> = cell
        .objects
        .iter()
        .filter(|o| o.is_actor)
        .map(|o| o.base_form_id)
        .collect();
    if wanted.is_empty() {
        return;
    }
    let mut npcs: HashMap<u32, &dovah_esm::Record> = HashMap::new();
    let mut classes: HashMap<u32, &dovah_esm::Record> = HashMap::new();
    let mut races: HashMap<u32, &dovah_esm::Record> = HashMap::new();
    for record in plugin.0.all_records() {
        if record.record_type == *b"NPC_" && wanted.contains(&record.form_id) {
            npcs.insert(record.form_id, record);
        } else if record.record_type == *b"CLAS" {
            classes.insert(record.form_id, record);
        } else if record.record_type == *b"RACE" {
            races.insert(record.form_id, record);
        }
    }
    let mut dead = 0;
    for object in &cell.objects {
        if !object.is_actor {
            continue;
        }
        let (level, max_health) = match npcs.get(&object.base_form_id) {
            Some(npc) => {
                let class_id = npc.field(b"CNAM").and_then(|f| u32_at(&f.data, 0));
                let race_id = npc.field(b"RNAM").and_then(|f| u32_at(&f.data, 0));
                npc_health_from_records(
                    npc,
                    class_id.and_then(|c| classes.get(&c).copied()),
                    race_id.and_then(|r| races.get(&r).copied()),
                    PLAYER_LEVEL,
                )
            }
            // No NPC_ record (shouldn't happen — the loader only
            // marks actors with one): level 1, 50 HP.
            None => (1, 50),
        };
        let is_dead = world.dead_npcs.contains(&object.ref_form_id);
        if is_dead {
            dead += 1;
        }
        combatants.npcs.insert(
            object.ref_form_id,
            Combatant {
                base_form_id: object.base_form_id,
                name: object.base_edid.clone(),
                position: coords::to_bevy_position(object.position),
                level,
                max_health,
                health: if is_dead { 0 } else { max_health },
                dead: is_dead,
            },
        );
    }
    crate::startup_log::log(&format!(
        "combatants built: {} actors ({} already dead)",
        combatants.npcs.len(),
        dead
    ));
}

/// Fall every part of one NPC rigidly about the feet (the
/// death pose). All parts of an actor share the placement
/// transform, so the same local pitch on each part entity
/// rotates the whole body as one rigid piece. Generic over the
/// query filter: `melee_attack` passes a `Without<Player>`
/// query (B0001), `apply_dead_npcs` an unfiltered one.
pub fn fall_npc_parts<F: QueryFilter>(
    parts: &mut Query<(&NpcPart, &mut Transform), F>,
    ref_form_id: u32,
) {
    for (part, mut transform) in parts.iter_mut() {
        if part.ref_form_id == ref_form_id {
            transform.rotation *= Quat::from_rotation_x(FALL_PITCH);
        }
    }
}

/// Re-apply the death fall for NPCs that died in an earlier
/// scene (OnEnter InGame, after `build_combatants`): the
/// parts are fresh entities each load, so they spawn standing
/// and must be fallen again.
pub fn apply_dead_npcs(combatants: Res<Combatants>, mut parts: Query<(&NpcPart, &mut Transform)>) {
    for (ref_form_id, npc) in &combatants.npcs {
        if npc.dead {
            fall_npc_parts(&mut parts, *ref_form_id);
        }
    }
}

// ----------------------------------------------------------------
// The attack system.
// ----------------------------------------------------------------

/// The melee attack (Update, InGame). Input: touch ATTACK R/L
/// buttons (edge-triggered), desktop mouse Left (only while
/// the cursor is grabbed — the re-grab click must not swing),
/// or the `--melee-test` queue. Both hands swing the same
/// equipped weapon — the inventory holds ONE equipped weapon,
/// so there is no left/right-hand distinction yet (vanilla has
/// one; dual-wield is a follow-up). No attack while a menu is
/// open. A swing with no target in range is a silent whiff.
#[allow(clippy::too_many_arguments)]
pub fn melee_attack(
    time: Res<Time>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    touch: Option<Res<TouchInputState>>,
    windows: Query<&CursorOptions, With<Window>>,
    player: Query<&Transform, With<Player>>,
    mut combatants: ResMut<Combatants>,
    mut cooldown: ResMut<SwingCooldown>,
    mut world: ResMut<WorldChanges>,
    inventory: Res<Inventory>,
    catalog: Res<ItemCatalog>,
    // Without<Player>: the player query above also reads
    // Transform, and Bevy's B0001 validation needs the two
    // Transform queries provably disjoint (a part is never
    // the player).
    mut parts: Query<(&NpcPart, &mut Transform), Without<Player>>,
    mut queue: Option<ResMut<MeleeTestQueue>>,
    dialogue_open: Option<Res<DialogueOpen>>,
    inventory_open: Option<Res<InventoryOpen>>,
    pause_open: Option<Res<PauseOpen>>,
    mut press_log: Option<ResMut<crate::touch::PressLog>>,
) {
    // The cooldown ticks even while a menu is open (the game
    // does not freeze on pause — the menu is UI, not state).
    cooldown.remaining = (cooldown.remaining - time.delta_secs()).max(0.0);
    if dialogue_open.map(|o| o.0).unwrap_or(false)
        || inventory_open.map(|o| o.0).unwrap_or(false)
        || pause_open.map(|o| o.0).unwrap_or(false)
    {
        return;
    }
    let mouse_attack = mouse
        .as_deref()
        .map(|m| m.just_pressed(MouseButton::Left))
        .unwrap_or(false)
        && windows
            .single()
            .map(|c| c.grab_mode == CursorGrabMode::Locked)
            .unwrap_or(false);
    let touch_right = touch
        .as_deref()
        .map(|t| t.attack_right_pressed)
        .unwrap_or(false);
    let touch_left = touch
        .as_deref()
        .map(|t| t.attack_left_pressed)
        .unwrap_or(false);
    let touch_attack = touch_right || touch_left;
    let queued = queue
        .as_deref()
        .map(|q| q.attacks_remaining > 0)
        .unwrap_or(false);
    if !mouse_attack && !touch_attack && !queued {
        return;
    }
    if cooldown.remaining > 0.0 {
        // Logging only: a press during the cooldown lands no
        // hit and previously logged nothing.
        log_attack_presses(
            &mut press_log,
            touch_right,
            touch_left,
            &format!("on cooldown ({:.1}s left)", cooldown.remaining),
        );
        return;
    }
    // Bows don't swing: an equipped bow means the ATTACK press
    // belongs to archery (`archery_fire`), not to this system.
    // The press flags are read-only — nothing is consumed here.
    if inventory
        .equipped_weapon
        .and_then(|id| catalog.defs.get(&id))
        .map(|def| def.is_bow)
        .unwrap_or(false)
    {
        // Logging only: the press went to archery, not melee.
        log_attack_presses(&mut press_log, touch_right, touch_left, "bow equipped (archery)");
        return;
    }
    let Ok(transform) = player.single() else {
        return;
    };
    // The weapon: the equipped catalog def when it is a
    // weapon, else fists. (The equip path only equips weapons
    // into this slot, but a stale/missing def must not panic.)
    let (damage, speed, reach_stat, weapon_name) = match inventory
        .equipped_weapon
        .and_then(|id| catalog.defs.get(&id))
    {
        Some(def) if def.kind == ItemKind::Weapon => {
            (def.damage, def.speed, def.reach, def.name.clone())
        }
        _ => (
            UNARMED_DAMAGE,
            UNARMED_SPEED,
            UNARMED_REACH,
            "fists".to_string(),
        ),
    };
    // The swing happens (cooldown spent, queue consumed) even
    // when it hits nothing — a whiff is still a swing.
    cooldown.remaining = swing_cooldown_secs(speed);
    if let Some(q) = queue.as_deref_mut() {
        if q.attacks_remaining > 0 {
            q.attacks_remaining -= 1;
        }
    }
    let feet = Vec3::new(
        transform.translation.x,
        transform.translation.y - EYE_HEIGHT,
        transform.translation.z,
    );
    let forward = *transform.forward();
    let reach_m = melee_reach_m(reach_stat);
    let Some(target_ref) = nearest_combatant_in_range(&combatants, feet, forward, reach_m) else {
        // Logging only: the no-target case — a press that lands
        // no hit previously logged nothing (the gap Dylan's
        // press-counting order closes).
        log_attack_presses(&mut press_log, touch_right, touch_left, "no target in range");
        return;
    };
    let Some(target) = combatants.npcs.get_mut(&target_ref) else {
        return;
    };
    let (new_health, died) = apply_damage(target.health, damage);
    target.health = new_health;
    if died {
        target.dead = true;
        world.dead_npcs.insert(target_ref);
        fall_npc_parts(&mut parts, target_ref);
        log_attack_presses(
            &mut press_log,
            touch_right,
            touch_left,
            &format!("killed {}", target.name),
        );
        crate::startup_log::log(&format!(
            "Player killed {} with {} ({} damage)",
            target.name, weapon_name, damage
        ));
    } else {
        log_attack_presses(
            &mut press_log,
            touch_right,
            touch_left,
            &format!("hit {} for {}", target.name, damage),
        );
        crate::startup_log::log(&format!(
            "Hit {} for {} ({}) — HP {}/{}",
            target.name, damage, weapon_name, new_health, target.max_health
        ));
    }
}

/// Press-log one swing's outcome for each pressed attack button
/// (logging only — the carve-out for Dylan's press-counting
/// order; no behavior change). Mouse/queued attacks find no
/// pending press and log nothing (touch buttons only). Both
/// buttons pressed in one frame log one line each — one line
/// per press.
fn log_attack_presses(
    press_log: &mut Option<ResMut<crate::touch::PressLog>>,
    right: bool,
    left: bool,
    outcome: &str,
) {
    let Some(log) = press_log.as_deref_mut() else {
        return;
    };
    for (pressed, button) in [
        (right, crate::touch::PressButton::AttackRight),
        (left, crate::touch::PressButton::AttackLeft),
    ] {
        if pressed {
            if let Some(count) = log.claim(button) {
                crate::startup_log::log(&crate::touch::PressLog::format_line(
                    button,
                    count,
                    Some(outcome),
                ));
            }
        }
    }
}

// ----------------------------------------------------------------
// The `--melee-test` seed.
// ----------------------------------------------------------------

/// Apply the `--melee-test` seed (OnEnter InGame, after
/// `build_combatants`): teleport the player in front of the
/// seeded NPC (Orgnar when present, else the nearest
/// combatant) and queue the scripted attacks. The teleport
/// sets the Transform AND the LookAngles — the screenshot
/// path has no player plugin to rebuild the rotation from the
/// angles, and the windowed path rebuilds it from the angles.
pub fn apply_melee_test_seed(
    mut commands: Commands,
    seed: Option<Res<MeleeTestSeed>>,
    combatants: Option<Res<Combatants>>,
    mut player: Query<(&mut Transform, &mut crate::player::LookAngles), With<Player>>,
) {
    let Some(seed) = seed else {
        return;
    };
    let (Some(combatants), Ok((mut transform, mut angles))) = (combatants, player.single_mut())
    else {
        commands.remove_resource::<MeleeTestSeed>();
        return;
    };
    // The target: the seeded base form ID, else the nearest
    // combatant to the player's current feet.
    let feet = Vec3::new(
        transform.translation.x,
        transform.translation.y - EYE_HEIGHT,
        transform.translation.z,
    );
    let target_ref = seed
        .npc_base
        .and_then(|base| {
            combatants
                .npcs
                .iter()
                .find(|(_, npc)| npc.base_form_id == base)
                .map(|(ref_form_id, _)| *ref_form_id)
        })
        .or_else(|| {
            combatants
                .npcs
                .iter()
                .filter(|(_, npc)| !npc.dead)
                .min_by(|(_, a), (_, b)| {
                    let da = (a.position - feet).length_squared();
                    let db = (b.position - feet).length_squared();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(ref_form_id, _)| *ref_form_id)
        });
    let Some(target_ref) = target_ref else {
        commands.remove_resource::<MeleeTestSeed>();
        return;
    };
    let target = &combatants.npcs[&target_ref];
    let target_pos = target.position;
    let target_name = target.name.clone();
    let target_level = target.level;
    let target_health = target.max_health;
    // Approach from the player's side, 1.2 m out (inside fist
    // reach), eye height, facing the target's chest.
    let mut dir = Vec3::new(feet.x - target_pos.x, 0.0, feet.z - target_pos.z);
    if dir.length_squared() < 0.01 {
        dir = Vec3::X;
    }
    dir = dir.normalize();
    let new_feet = target_pos + dir * MELEE_TEST_DISTANCE_M;
    let eye = new_feet + Vec3::Y * EYE_HEIGHT;
    let look_at = target_pos + Vec3::Y * 1.2;
    *transform = Transform::from_translation(eye).looking_at(look_at, Vec3::Y);
    *angles = crate::player::LookAngles::from_forward(look_at - eye);
    commands.insert_resource(MeleeTestQueue {
        attacks_remaining: seed.attacks,
    });
    crate::startup_log::log(&format!(
        "melee-test: player at ({:.2}, {:.2}, {:.2}) facing {} (ref 0x{:08x}, level {}, {} HP), {} attacks queued",
        eye.x, eye.y, eye.z, target_name, target_ref, target_level, target_health, seed.attacks
    ));
    commands.remove_resource::<MeleeTestSeed>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use dovah_esm::{Field, Record};

    fn field(code: &[u8; 4], data: Vec<u8>) -> Field {
        Field {
            field_type: *code,
            data,
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

    fn id_field(code: &[u8; 4], form_id: u32) -> Field {
        field(code, form_id.to_le_bytes().to_vec())
    }

    /// An NPC_ record with the given ACBS bytes (24) + class +
    /// race links. The ACBS bytes are the REAL bytes from the
    /// melee_probe (Skyrim.esm).
    fn npc(form_id: u32, acbs: [u8; 24], class: u32, race: u32) -> Record {
        record(
            b"NPC_",
            form_id,
            vec![
                field(b"ACBS", acbs.to_vec()),
                id_field(b"CNAM", class),
                id_field(b"RNAM", race),
            ],
        )
    }

    /// A CLAS record with the given attribute weights (DATA
    /// bytes 32-34).
    fn class(form_id: u32, wh: u8, wm: u8, ws: u8) -> Record {
        let mut data = vec![0u8; 36];
        data[32] = wh;
        data[33] = wm;
        data[34] = ws;
        record(b"CLAS", form_id, vec![field(b"DATA", data)])
    }

    /// A RACE record with the given starting health (DATA f32
    /// @36).
    fn race(form_id: u32, health: f32) -> Record {
        let mut data = vec![0u8; 164];
        data[36..40].copy_from_slice(&health.to_le_bytes());
        record(b"RACE", form_id, vec![field(b"DATA", data)])
    }

    // Real ACBS bytes from the probe (Skyrim.esm):
    // Orgnar: flags 0x00020030, level 4, min 0, max 100, offset 0.
    const ORGNAR_ACBS: [u8; 24] = [
        0x30, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x64, 0x00, 0x64,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    // Delphine: flags 0x102800b3, field 900 (0.9x), min 10,
    // max 30, offset 50.
    const DELPHINE_ACBS: [u8; 24] = [
        0xb3, 0x00, 0x28, 0x10, 0x00, 0x00, 0x32, 0x00, 0x84, 0x03, 0x0a, 0x00, 0x1e, 0x00, 0x64,
        0x00, 0x23, 0x00, 0x00, 0x00, 0x32, 0x00, 0x00, 0x00,
    ];
    // Chicken: flags 0 (no auto-calc), level 1, offset 0.
    const CHICKEN_ACBS: [u8; 24] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    #[test]
    fn health_formula_matches_vanilla_values() {
        let nord = race(0x0001_3746, 50.0);
        let chicken_race = race(0x000a_919d, 5.0);
        // Orgnar: 50 + 0 + 5*3 + 10*3*1/3 = 75.
        let (level, health) = npc_health_from_records(
            &npc(0x0001_3479, ORGNAR_ACBS, 0x0001_3256, 0x0001_3746),
            Some(&class(0x0001_3256, 1, 1, 1)),
            Some(&nord),
            1,
        );
        assert_eq!(level, 4);
        assert_eq!(health, 75);
        // Delphine: level 10 (0.9x clamped to min 10),
        // 50 + 50 + 5*9 + 10*9*3/6 = 190.
        let breton = race(0x0001_3741, 50.0);
        let (level, health) = npc_health_from_records(
            &npc(0x0001_3478, DELPHINE_ACBS, 0x0002_1a74, 0x0001_3741),
            Some(&class(0x0002_1a74, 3, 3, 0)),
            Some(&breton),
            1,
        );
        assert_eq!(level, 10);
        assert_eq!(health, 190);
        // Chicken: no auto-calc, racial 5, no offset = 5.
        let (level, health) = npc_health_from_records(
            &npc(0x000a_91a0, CHICKEN_ACBS, 0x0001_ce3d, 0x000a_919d),
            Some(&class(0x0001_ce3d, 3, 0, 3)),
            Some(&chicken_race),
            1,
        );
        assert_eq!(level, 1);
        assert_eq!(health, 5);
    }

    #[test]
    fn pc_level_mult_clamps_to_calc_range() {
        // Delphine at player level 20: 0.9 * 20 = 18.
        assert_eq!(npc_level(0x80, 900, 10, 30, 20), 18);
        // At player level 50: 45, clamped to max 30.
        assert_eq!(npc_level(0x80, 900, 10, 30, 50), 30);
        // Fixed level ignores the player level.
        assert_eq!(npc_level(0x10, 4, 0, 100, 50), 4);
    }

    #[test]
    fn melee_range_requires_arc_and_reach() {
        let feet = Vec3::new(0.0, 0.0, 0.0);
        let forward = Vec3::new(0.0, 0.0, -1.0);
        // 1 m ahead: in range.
        assert!(in_melee_range(
            Vec3::new(0.0, 0.0, -1.0),
            feet,
            forward,
            1.5
        ));
        // 1 m behind: out of arc.
        assert!(!in_melee_range(
            Vec3::new(0.0, 0.0, 1.0),
            feet,
            forward,
            1.5
        ));
        // 2 m ahead: out of reach.
        assert!(!in_melee_range(
            Vec3::new(0.0, 0.0, -2.0),
            feet,
            forward,
            1.5
        ));
        // 1 m ahead but 4 m up: 2.3 m above the eye, out of
        // the 2.0 m vertical tolerance.
        assert!(!in_melee_range(
            Vec3::new(0.0, 4.0, -1.0),
            feet,
            forward,
            1.5
        ));
        // Point-blank: always in arc.
        assert!(in_melee_range(Vec3::new(0.0, 0.0, 0.0), feet, forward, 1.5));
        // 45 degrees off-axis is the boundary (dot 0.7071).
        let diag = Vec3::new(-1.0, 0.0, -1.0).normalize();
        assert!(in_melee_range(diag, feet, forward, 1.5));
        // 90 degrees off-axis: out of arc.
        assert!(!in_melee_range(
            Vec3::new(-1.0, 0.0, 0.0),
            feet,
            forward,
            1.5
        ));
    }

    #[test]
    fn nearest_combatant_skips_dead_and_picks_closest() {
        let mut combatants = Combatants::default();
        let mk = |name: &str, pos: Vec3, dead: bool| Combatant {
            base_form_id: 0,
            name: name.to_string(),
            position: pos,
            level: 4,
            max_health: 75,
            health: if dead { 0 } else { 75 },
            dead,
        };
        // A dead NPC 0.5 m ahead (must be skipped), a live one
        // 1 m ahead, a live one 1.4 m ahead.
        combatants
            .npcs
            .insert(0x1, mk("Dead", Vec3::new(0.0, 0.0, -0.5), true));
        combatants
            .npcs
            .insert(0x2, mk("Near", Vec3::new(0.0, 0.0, -1.0), false));
        combatants
            .npcs
            .insert(0x3, mk("Far", Vec3::new(0.0, 0.0, -1.4), false));
        let feet = Vec3::ZERO;
        let forward = Vec3::new(0.0, 0.0, -1.0);
        assert_eq!(
            nearest_combatant_in_range(&combatants, feet, forward, 1.5),
            Some(0x2)
        );
        // Nothing in range (all behind).
        let behind = Vec3::new(0.0, 0.0, 1.0);
        assert_eq!(
            nearest_combatant_in_range(&combatants, feet, behind, 1.5),
            None
        );
    }

    #[test]
    fn swing_cooldown_from_speed() {
        assert!((swing_cooldown_secs(1.0) - 1.0).abs() < 0.001);
        assert!((swing_cooldown_secs(1.3) - 0.769).abs() < 0.001);
        assert!((swing_cooldown_secs(0.7) - 1.429).abs() < 0.001);
        // Zero/negative speed cannot divide by zero.
        assert!((swing_cooldown_secs(0.0) - 10.0).abs() < 0.001);
        assert!((melee_reach_m(1.0) - 1.5).abs() < 0.001);
        assert!((melee_reach_m(0.7) - 1.05).abs() < 0.001);
        assert!((melee_reach_m(1.3) - 1.95).abs() < 0.001);
    }

    #[test]
    fn damage_kills_at_zero() {
        assert_eq!(apply_damage(75, 4), (71, false));
        assert_eq!(apply_damage(4, 4), (0, true));
        assert_eq!(apply_damage(3, 4), (0, true));
    }

    /// The full loop headless: 19 queued fist attacks kill
    /// Orgnar (75 HP / 4 damage), the ref lands in
    /// WorldChanges.dead_npcs, and the part entity falls 90
    /// degrees. Runs through the REAL `melee_attack` system.
    #[test]
    fn melee_attack_damages_and_kills_through_the_real_system() {
        use bevy::app::App;
        use bevy::prelude::MinimalPlugins;
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // Manual 1.1 s steps: each update after the first
        // advances virtual time by 1.1 s, but Bevy clamps the
        // reported frame delta to 250 ms — so the 1.0 s fist
        // cooldown needs four updates to elapse (see the loop
        // below).
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            1100,
        )));
        app.add_systems(bevy::app::Update, melee_attack);
        let mut combatants = Combatants::default();
        combatants.npcs.insert(
            0x0001_3486,
            Combatant {
                base_form_id: 0x0001_3479,
                name: "Orgnar".to_string(),
                position: Vec3::new(0.0, 0.0, -1.0),
                level: 4,
                max_health: 75,
                health: 75,
                dead: false,
            },
        );
        app.insert_resource(combatants);
        app.insert_resource(SwingCooldown::default());
        app.insert_resource(WorldChanges::default());
        app.insert_resource(Inventory::default());
        app.insert_resource(ItemCatalog::default());
        app.insert_resource(MeleeTestQueue {
            attacks_remaining: 19,
        });
        // The player at eye height, facing -Z (Bevy default
        // forward), 1 m from Orgnar.
        app.world_mut().spawn((
            Player,
            Transform::from_translation(Vec3::new(0.0, EYE_HEIGHT, 0.0)),
        ));
        // One part entity for Orgnar (the fall target).
        let part = app
            .world_mut()
            .spawn((
                NpcPart {
                    ref_form_id: 0x0001_3486,
                },
                Transform::default(),
            ))
            .id();
        // First update reports delta 0 (no previous instant),
        // so it lands hit 1 and sets the 1.0 s cooldown; each
        // later update advances 1.1 s of virtual time — but
        // Bevy clamps frame deltas to 250 ms, so the cooldown
        // needs FOUR updates to elapse. Hits land on updates
        // 1, 5, 9, ..., 73 (19 hits); 80 updates gives margin.
        for _ in 0..80 {
            app.update();
        }
        let combatants = app.world().resource::<Combatants>();
        let orgnar = &combatants.npcs[&0x0001_3486];
        assert!(orgnar.dead);
        assert_eq!(orgnar.health, 0);
        let world = app.world().resource::<WorldChanges>();
        assert!(world.dead_npcs.contains(&0x0001_3486));
        // The part fell 90 degrees about local X: its up axis
        // (+Y) now points along +Z (behind).
        let transform = app.world().get::<Transform>(part).unwrap();
        let up = transform.rotation * Vec3::Y;
        assert!((up - Vec3::Z).length() < 0.001);
        // The queue drained exactly 19 swings.
        let queue = app.world().resource::<MeleeTestQueue>();
        assert_eq!(queue.attacks_remaining, 0);
    }

    /// The seed teleports the player 1.2 m from Orgnar,
    /// facing him, and queues the attacks.
    #[test]
    fn melee_test_seed_teleports_in_front_of_the_target() {
        use bevy::app::App;

        let mut app = App::new();
        app.add_systems(bevy::app::Startup, apply_melee_test_seed);
        let mut combatants = Combatants::default();
        combatants.npcs.insert(
            0x0001_3486,
            Combatant {
                base_form_id: 0x0001_3479,
                name: "Orgnar".to_string(),
                position: Vec3::new(-18.75, 0.0, -0.26),
                level: 4,
                max_health: 75,
                health: 75,
                dead: false,
            },
        );
        app.insert_resource(combatants);
        app.insert_resource(MeleeTestSeed {
            attacks: 5,
            npc_base: Some(0x0001_3479),
        });
        // The player spawns near the inn door.
        let player_entity = app
            .world_mut()
            .spawn((
                Player,
                Transform::from_translation(Vec3::new(-14.5, EYE_HEIGHT, -0.3)),
                crate::player::LookAngles {
                    yaw: 0.0,
                    pitch: 0.0,
                },
            ))
            .id();
        app.update();
        let transform = app.world().get::<Transform>(player_entity).unwrap();
        let feet = Vec3::new(
            transform.translation.x,
            transform.translation.y - EYE_HEIGHT,
            transform.translation.z,
        );
        let target = Vec3::new(-18.75, 0.0, -0.26);
        // 1.2 m from Orgnar (inside fist reach).
        assert!((feet.distance(target) - MELEE_TEST_DISTANCE_M).abs() < 0.01);
        // Facing him: the forward dot with the direction to
        // the target is ~1.
        let to_target = (target + Vec3::Y * 1.2 - transform.translation).normalize();
        assert!(transform.forward().dot(to_target) > 0.99);
        let queue = app.world().resource::<MeleeTestQueue>();
        assert_eq!(queue.attacks_remaining, 5);
        // The seed is consumed.
        assert!(app.world().get_resource::<MeleeTestSeed>().is_none());
    }
}
