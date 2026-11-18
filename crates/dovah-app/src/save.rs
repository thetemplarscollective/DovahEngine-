//! The save system: a versioned JSON save file, the pause
//! screen's Save/Load panels, and the spawn override that puts
//! the player back where they stood.
//!
//! The format lives here and ONLY here. `SaveData` is
//! everything the loader needs: where the player stood (cell +
//! eye position + facing), what they carried (inventory, gold,
//! equipped gear), their state (stamina, level, equipped spell,
//! skill levels, settings), and what they changed in the world
//! (doors left open, items picked up, containers looted).
//!
//! What a save is NOT: Bethesda data. A save holds the
//! PLAYER's own state — positions, form IDs, counts — which
//! the game rebuilds the world from at load time. No meshes,
//! textures, or record text are ever written.
//!
//! ## The load path (the spawn override)
//!
//! Loading reuses the door-transition machinery exactly: the
//! save's cell becomes a `SceneTarget`, the save's position +
//! yaw become a `SpawnPoint`, and `load_scene` loads the cell
//! on a worker thread. The spawn point flows into
//! `CellData.spawn`, and `spawn_camera` stands the player at
//! it — the same path a door arrival takes, so no scene.rs
//! change was needed: the override point already existed, the
//! save system just drives it. Pitch is not expressible in a
//! `SpawnPoint` (door arrivals look level), so the restore
//! re-asserts the full transform + `LookAngles` on the first
//! InGame frame.
//!
//! Door states and world changes are applied BEFORE the load
//! starts (at trigger time): `begin_spawning` seeds open doors
//! from `DoorMemory`, and `apply_world_changes` (in the
//! OnEnter(InGame) chain, after `build_interactables`) removes
//! picked-up items and looted containers from the fresh
//! indexes — so a loaded game never re-spawns what the player
//! already took. The same pass runs on every door transition,
//! which fixes the transition dupe too (a picked-up item used
//! to come back when the player re-entered the cell).
//!
//! ## What is NOT saved (honest gaps)
//!
//! * Quest stages — the quest index has no state yet (no
//!   quest system tracks active/completed), so there is
//!   nothing to save. The journal lists quests; progress is
//!   not tracked.
//! * NPC positions — NPCs spawn from their cell refs at their
//!   placed positions. Nobody moves yet (no AI schedules), so
//!   this is invisible today.
//! * Dropped items — a dropped item's ground model is spawned
//!   under a synthetic ref ID that does not survive a scene
//!   load. Dropping an item, leaving, and coming back loses
//!   it. (A later milestone can persist drops as
//!   base-form-ID + position.)
//! * Health / magicka — no damage or magic system exists, so
//!   both are always full. The format carries the fields (100)
//!   for when those systems land.
//! * Skill progression — skill levels are derived from the
//!   race record (no XP system). The save round-trips the
//!   levels so the plumbing is ready; today they always equal
//!   the race defaults.
//! * Known spells — derived from the player + race SPLO
//!   records (no spell-learning system exists). Only the
//!   EQUIPPED spell is saved; the known set is recomputed.
//! * Game date — no time system advances the clock, so the
//!   date is always the start date ("17th of Last Seed, 4E
//!   201"). The format carries it for when time moves.

use crate::cell::{CellData, SceneTarget, SpawnPoint};
use crate::character::{CharacterState, SkillIndex};
use crate::coords;
use crate::dialogue::GameStrings;
use crate::doors::DoorMemory;
use crate::interact::{Containers, Pickupables};
use crate::inventory::{prettify_edid, EquipSlot, Inventory, InventoryStack};
use crate::land::LandData;
use crate::pause::GameSettings;
use crate::player::{LookAngles, Stamina, EYE_HEIGHT};
use crate::scene::{Player, SceneEntity};
use bevy::app::{App, Plugin, Update};
use bevy::ecs::prelude::*;
use bevy::ecs::system::SystemParam;
use bevy::math::{EulerRot, Quat, Vec3};
use bevy::state::prelude::NextState;
use bevy::transform::components::Transform;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Save format version. Bump when the fields change; the
/// loader rejects unknown versions rather than guessing.
/// v1 (UI milestone 1) had no spells, skills, doors, world
/// changes, health/magicka, game date, or exterior grid — v1
/// files fail cleanly with a version error.
pub const SAVE_FORMAT_VERSION: u32 = 2;

/// The autosave slot: one fixed file, overwritten on every
/// door transition (Skyrim autosaves on travel).
pub const AUTOSAVE_FILENAME: &str = "save-autosave.json";

/// Save file prefix: `save-<unix>.json` (manual saves) and
/// `save-autosave.json` (the autosave slot).
const SAVE_PREFIX: &str = "save-";

/// Where save files + settings.json live. Inserted by lib.rs
/// from the data dir (`<data>/dovahengine-saves`).
#[derive(Debug, Resource)]
pub struct SaveDir(pub PathBuf);

/// One save file's contents: everything the loader needs to
/// put the player back where they stood, with what they
/// carried, in the world as they left it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveData {
    pub version: u32,
    /// Cell EDID (interior) or the exterior block's centre
    /// cell EDID.
    pub cell_edid: String,
    pub cell_form_id: u32,
    pub interior: bool,
    /// Exterior centre grid (the 3x3 block `load_exterior`
    /// loads around). `None` for interiors.
    pub exterior_grid: Option<(i32, i32)>,
    /// Display location (the CELL's real name, or the
    /// prettified EDID when it has none).
    pub location: String,
    /// Eye position, Bevy metres.
    pub position: [f32; 3],
    /// Bevy look yaw/pitch (`LookAngles` convention: yaw is
    /// the YXZ Euler yaw, the NEGATIVE of the Skyrim yaw — see
    /// `bevy_yaw_to_skyrim_yaw`).
    pub yaw: f32,
    pub pitch: f32,
    /// Always 100 today (no damage/magic systems) — the fields
    /// exist so the format does not need a bump when they land.
    pub health: f32,
    pub magicka: f32,
    pub stamina: f32,
    /// Always 1 today (no XP system).
    pub level: u32,
    pub gold: u32,
    /// (base form ID, count) — gold is NOT in here (it has its
    /// own counter, like the inventory).
    pub stacks: Vec<(u32, i32)>,
    pub equipped_weapon: Option<u32>,
    /// (slot name, base form ID) — slot names are the
    /// `EquipSlot` variant names ("Head", "Body", "Hands",
    /// "Feet", "Shield").
    pub equipped_armor: Vec<(String, u32)>,
    /// The equipped spell's SPEL form ID (menu state — casting
    /// is a later milestone). The KNOWN set is not saved: it is
    /// derived from the player + race SPLO records.
    pub equipped_spell: Option<u32>,
    /// (AVIF skill index 6-23, level) — derived from the race
    /// record today, round-tripped so progression has a home.
    pub skill_levels: Vec<(u8, u8)>,
    /// Door ref form IDs the player left open, sorted.
    pub open_doors: Vec<u32>,
    /// Loose-item ref form IDs the player picked up, sorted.
    pub picked_up: Vec<u32>,
    /// Container ref form IDs the player looted, sorted.
    pub looted_containers: Vec<u32>,
    /// The in-game date ("17th of Last Seed, 4E 201" — the
    /// start date, always, until a time system advances it).
    pub game_date: String,
    pub sensitivity: crate::pause::Sensitivity,
    pub invert_y: bool,
    /// Unix seconds, for sorting + display.
    pub saved_at_unix: u64,
    /// Display string, computed at write time ("Nov 15, 2026,
    /// 1:30 PM UTC") so the list never re-derives it.
    pub saved_at_display: String,
}

/// One save in the list (parsed header — the full file is
/// re-read by the loader).
#[derive(Debug, Clone)]
pub struct SaveInfo {
    pub path: PathBuf,
    pub location: String,
    pub level: u32,
    pub game_date: String,
    pub saved_at_unix: u64,
    pub saved_at_display: String,
}

/// The scanned save list, refreshed when a Load/Save panel
/// opens and after every write.
#[derive(Debug, Default, Resource)]
pub struct SaveList {
    pub saves: Vec<SaveInfo>,
}

/// A save was requested (E on the Save panel / the Save Game
/// button / the `--save-test` seed). Consumed by
/// `write_save_on_request`, which has the game-state access
/// the key handler does not.
#[derive(Debug, Default, Resource)]
pub struct SaveRequested(pub bool);

/// A load was requested, with the save file to load. Set by
/// the Load panel (E / the Load Game button) and by the
/// `--load-test` seed. Consumed by `load_save_on_request`.
#[derive(Debug, Default, Resource)]
pub struct LoadRequested(pub Option<PathBuf>);

/// The save being loaded: set at trigger time (before the
/// scene load starts), applied by `apply_pending_restore` on
/// the first InGame frame, then cleared.
#[derive(Debug, Default, Resource)]
pub struct PendingRestore(pub Option<SaveData>);

/// What the player changed in the world, by ref form ID.
/// Recorded by the pickup/container systems, saved with every
/// save, replaced by the save's sets on load, and re-applied
/// to the fresh indexes on every scene entry (loads AND door
/// transitions) — so a taken item stays taken.
#[derive(Debug, Default, Resource)]
pub struct WorldChanges {
    pub picked_up: HashSet<u32>,
    pub looted_containers: HashSet<u32>,
    /// NPCs the player killed, by ref form ID (melee). They
    /// spawn dead on every later scene entry (see
    /// `combat::build_combatants` + `apply_dead_npcs`).
    /// SESSION-ONLY: deliberately NOT in `SaveData` — that
    /// struct has no `#[serde(default)]`, so a new field would
    /// break old saves unless SAVE_FORMAT_VERSION is bumped.
    /// Save-file persistence of deaths is a follow-up.
    pub dead_npcs: HashSet<u32>,
}

/// The `--save-test` seed: write a save on the first InGame
/// frame (desktop proof hook — proves the save path writes a
/// real file with the live game state).
#[derive(Debug, Resource)]
pub struct SaveSeed;

/// The `--load-test <file>` seed: load the named save file
/// (a file name inside the save dir) on the first InGame
/// frame (desktop proof hook — proves the load path end to
/// end, from a different starting scene).
#[derive(Debug, Resource)]
pub struct LoadTestSeed {
    pub file: String,
}

/// Save file name for a unix timestamp: `save-<unix>.json`.
/// The timestamp sorts the files and never collides (two saves
/// in one second overwrite — acceptable; the game overwrites
/// same-name saves too).
fn save_filename(unix: u64) -> String {
    format!("{SAVE_PREFIX}{unix}.json")
}

/// Write a save file (creates the dir). Returns the path.
pub fn write_save(dir: &Path, data: &SaveData) -> std::io::Result<PathBuf> {
    write_save_as(dir, data, &save_filename(data.saved_at_unix))
}

/// Write the autosave slot (one fixed file, overwritten on
/// every door transition). Returns the path.
pub fn write_autosave(dir: &Path, data: &SaveData) -> std::io::Result<PathBuf> {
    write_save_as(dir, data, AUTOSAVE_FILENAME)
}

fn write_save_as(dir: &Path, data: &SaveData, name: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(name);
    let text = serde_json::to_string_pretty(data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Read + validate a save file: bad JSON and wrong versions
/// fail with a message, never a guess. A v1 file (UI
/// milestone 1) fails here — the fields changed, and loading
/// it as v2 would invent state. The version is probed BEFORE
/// the full parse: an old file lacks fields the current format
/// requires, so a full parse would fail with a confusing
/// missing-field error instead of the version rejection.
pub fn read_save(path: &Path) -> Result<SaveData, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    #[derive(Deserialize)]
    struct VersionProbe {
        version: u32,
    }
    let probe: VersionProbe = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a valid save file: {e}", path.display()))?;
    if probe.version != SAVE_FORMAT_VERSION {
        return Err(format!(
            "{} is save version {}, this build reads version {}",
            path.display(),
            probe.version,
            SAVE_FORMAT_VERSION
        ));
    }
    let data: SaveData = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a valid save file: {e}", path.display()))?;
    Ok(data)
}

/// List the saves in a dir, newest first. Unparseable files
/// (wrong version, corrupt JSON) are skipped, never fatal —
/// a bad save must not hide the good ones.
pub fn list_saves(dir: &Path) -> Vec<SaveInfo> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(SAVE_PREFIX) || !name.ends_with(".json") {
            continue;
        }
        let Ok(data) = read_save(&path) else {
            continue;
        };
        out.push(SaveInfo {
            path,
            location: data.location,
            level: data.level,
            game_date: data.game_date,
            saved_at_unix: data.saved_at_unix,
            saved_at_display: data.saved_at_display,
        });
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.saved_at_unix));
    out
}

/// Unix seconds -> "Nov 15, 2026, 1:30 PM UTC" (UTC — the
/// phone's zone is not available to the engine, so the label
/// says so). Pure — unit-tested. Civil-date math is Howard
/// Hinnant's days-from-civil inverse.
pub fn format_save_time(unix: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    // civil_from_days (Hinnant): days since 1970-01-01 -> y/m/d.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let hour24 = secs / 3600;
    let minute = (secs % 3600) / 60;
    let (hour12, ampm) = match hour24 {
        0 => (12, "AM"),
        1..=11 => (hour24, "AM"),
        12 => (12, "PM"),
        _ => (hour24 - 12, "PM"),
    };
    format!(
        "{} {}, {}, {}:{:02} {} UTC",
        MONTHS[(m - 1) as usize],
        d,
        y,
        hour12,
        minute,
        ampm
    )
}

/// Current unix seconds (0 when the clock is unavailable —
/// never panics on a missing clock).
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Pure conversions (unit-tested)
// ---------------------------------------------------------------------------

/// Bevy look yaw (`LookAngles`, YXZ Euler) -> Skyrim yaw
/// (clockwise-positive, the `SpawnPoint` convention). The two
/// are negated: `spawn_viewpoint` builds the Bevy forward
/// (sin s, 0, -cos s) from a Skyrim yaw s, and
/// `LookAngles::from_forward` reads yaw atan2(-f.x, -f.z) =
/// -s. So a saved Bevy yaw b becomes the Skyrim yaw -b.
pub fn bevy_yaw_to_skyrim_yaw(bevy_yaw: f32) -> f32 {
    -bevy_yaw
}

/// A save's spawn point: the eye position (Bevy metres) down
/// to the feet (minus eye height), converted to Skyrim units,
/// with the yaw flipped to the Skyrim convention.
/// `spawn_viewpoint` adds the eye height back and faces the
/// yaw — the round trip restores the exact saved view.
pub fn save_spawn_point(data: &SaveData) -> SpawnPoint {
    let feet = Vec3::new(
        data.position[0],
        data.position[1] - EYE_HEIGHT,
        data.position[2],
    );
    SpawnPoint {
        position: coords::to_skyrim_position(feet),
        yaw: bevy_yaw_to_skyrim_yaw(data.yaw),
    }
}

/// A save's load target: the interior by EDID, or the
/// exterior block around the saved centre grid. An exterior
/// save without a grid (a hand-written file) falls back to
/// Riverwood (5, -11) — the only exterior block the engine
/// knows — rather than failing the load.
pub fn save_target(data: &SaveData) -> SceneTarget {
    if data.interior {
        SceneTarget::Interior {
            form_id: data.cell_form_id,
            edid: data.cell_edid.clone(),
        }
    } else {
        SceneTarget::Exterior {
            grid: data.exterior_grid.unwrap_or((5, -11)),
        }
    }
}

/// The exterior block's centre grid, from its LAND cells: the
/// median grid of the (3x3) block. `None` for an interior
/// (no LAND) or an empty land list.
pub fn exterior_center_grid(lands: &[LandData]) -> Option<(i32, i32)> {
    if lands.is_empty() {
        return None;
    }
    let mut xs: Vec<i32> = lands.iter().map(|l| l.grid.0).collect();
    let mut ys: Vec<i32> = lands.iter().map(|l| l.grid.1).collect();
    xs.sort_unstable();
    ys.sort_unstable();
    Some((xs[xs.len() / 2], ys[ys.len() / 2]))
}

/// Parse an `EquipSlot` variant name back ("Head", "Body",
/// "Hands", "Feet", "Shield" — the `format!("{slot:?}")` the
/// capture writes). Unknown names (a future slot) are skipped,
/// never fatal.
fn parse_equip_slot(name: &str) -> Option<EquipSlot> {
    match name {
        "Head" => Some(EquipSlot::Head),
        "Body" => Some(EquipSlot::Body),
        "Hands" => Some(EquipSlot::Hands),
        "Feet" => Some(EquipSlot::Feet),
        "Shield" => Some(EquipSlot::Shield),
        _ => None,
    }
}

/// Sort a ref-ID set into the save's deterministic Vec form.
fn sorted(set: &HashSet<u32>) -> Vec<u32> {
    let mut out: Vec<u32> = set.iter().copied().collect();
    out.sort_unstable();
    out
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

/// Everything a save captures, bundled so the capture stays
/// one parameter (the save-write system and the autosave call
/// in `door_transition` share it — Bevy systems cap at 16
/// parameters, and a save touches twelve pieces of state).
/// Optional resources are absent on paths that lack them (the
/// screenshot path has no `Stamina` without `--screenshot-ui`,
/// no `DoorMemory` without `--door-open`); absent means the
/// honest default (full stamina, no open doors, no changes).
#[derive(SystemParam)]
pub struct SaveCaptureParams<'w, 's> {
    plugin: Option<Res<'w, crate::SharedPlugin>>,
    strings: Option<Res<'w, GameStrings>>,
    cell: Option<Res<'w, CellData>>,
    player: Query<'w, 's, (&'static Transform, &'static LookAngles), With<Player>>,
    inventory: Res<'w, Inventory>,
    stamina: Option<Res<'w, Stamina>>,
    settings: Res<'w, GameSettings>,
    doors: Option<Res<'w, DoorMemory>>,
    world: Option<Res<'w, WorldChanges>>,
    character: Option<Res<'w, CharacterState>>,
    skills: Option<Res<'w, SkillIndex>>,
}

impl SaveCaptureParams<'_, '_> {
    /// Build the save data for the current game state, or
    /// `None` when the game state is not available yet (a save
    /// before the first scene loads has nothing to capture).
    /// The location resolves the CELL's real name from the
    /// plugin + strings; the prettified EDID is the fallback
    /// (the same fallback M18 uses for NPC names).
    pub fn capture(&self) -> Option<SaveData> {
        let plugin = self.plugin.as_ref()?;
        let strings = self.strings.as_ref()?;
        let cell = self.cell.as_ref()?;
        let (transform, angles) = self.player.single().ok()?;
        let location = plugin
            .0
            .find(cell.form_id)
            .and_then(|record| strings.name_of(record))
            .unwrap_or_else(|| prettify_edid(&cell.edid));
        let unix = now_unix();
        Some(SaveData {
            version: SAVE_FORMAT_VERSION,
            cell_edid: cell.edid.clone(),
            cell_form_id: cell.form_id,
            interior: cell.interior,
            exterior_grid: exterior_center_grid(&cell.lands),
            location,
            position: transform.translation.to_array(),
            yaw: angles.yaw,
            pitch: angles.pitch,
            // No damage/magic systems yet — always full.
            health: 100.0,
            magicka: 100.0,
            stamina: self.stamina.as_deref().map(|s| s.value).unwrap_or(100.0),
            // No XP system yet — always level 1.
            level: 1,
            gold: self.inventory.gold,
            stacks: self
                .inventory
                .stacks
                .iter()
                .map(|s| (s.base_form_id, s.count))
                .collect(),
            equipped_weapon: self.inventory.equipped_weapon,
            equipped_armor: self
                .inventory
                .equipped_armor
                .iter()
                .map(|(slot, base)| (format!("{slot:?}"), *base))
                .collect(),
            equipped_spell: self.character.as_ref().and_then(|c| c.equipped_spell),
            skill_levels: self
                .skills
                .as_ref()
                .map(|s| s.skills.iter().map(|e| (e.index, e.level)).collect())
                .unwrap_or_default(),
            open_doors: self
                .doors
                .as_ref()
                .map(|d| sorted(&d.open))
                .unwrap_or_default(),
            picked_up: self
                .world
                .as_ref()
                .map(|w| sorted(&w.picked_up))
                .unwrap_or_default(),
            looted_containers: self
                .world
                .as_ref()
                .map(|w| sorted(&w.looted_containers))
                .unwrap_or_default(),
            game_date: crate::pause::GAME_START_DATE.to_string(),
            sensitivity: self.settings.sensitivity,
            invert_y: self.settings.invert_y,
            saved_at_unix: unix,
            saved_at_display: format_save_time(unix),
        })
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Write the requested save, rescan the list, rebuild the UI.
/// Skips (clearing the request) when the game state is not
/// available yet — a save before the first scene loads has
/// nothing to capture.
fn write_save_on_request(
    mut requested: ResMut<SaveRequested>,
    mut save_list: ResMut<SaveList>,
    save_dir: Option<Res<SaveDir>>,
    capture: SaveCaptureParams,
    mut dirty: ResMut<crate::pause::PauseDirty>,
) {
    if !requested.0 {
        return;
    }
    requested.0 = false;
    let Some(dir) = save_dir else {
        crate::startup_log::log("save skipped: no save dir");
        return;
    };
    let Some(data) = capture.capture() else {
        crate::startup_log::log("save skipped: game state not ready");
        return;
    };
    match write_save(&dir.0, &data) {
        Ok(path) => {
            crate::startup_log::log(&format!("saved: {}", path.display()));
            save_list.saves = list_saves(&dir.0);
            dirty.0 = true;
        }
        Err(e) => {
            crate::startup_log::log(&format!("save failed: {e}"));
        }
    }
}

/// Write the autosave slot from the live game state (the
/// door-transition hook in lib.rs calls this through the
/// capture bundle). Best-effort: a failed autosave logs and
/// the transition carries on — an autosave must never block
/// walking through a door.
pub fn write_autosave_now(dir: &Path, capture: &SaveCaptureParams) {
    let Some(data) = capture.capture() else {
        crate::startup_log::log("autosave skipped: game state not ready");
        return;
    };
    match write_autosave(dir, &data) {
        Ok(path) => {
            crate::startup_log::log(&format!("autosaved: {}", path.display()));
        }
        Err(e) => {
            crate::startup_log::log(&format!("autosave failed: {e}"));
        }
    }
}

/// Load the requested save: read + validate the file, set the
/// door memory + world changes from it (BEFORE the load — the
/// spawner seeds open doors from the memory, and the
/// world-changes pass runs on scene entry), store the restore,
/// then start the same worker-thread load a door transition
/// uses, with the save's cell as the target and the save's
/// position as the spawn point. A bad file logs and stays in
/// the game — a corrupt save must never strand the player on a
/// loading screen.
#[allow(clippy::too_many_arguments)]
fn load_save_on_request(
    mut requested: ResMut<LoadRequested>,
    plugin: Option<Res<crate::SharedPlugin>>,
    mut door_memory: Option<ResMut<DoorMemory>>,
    mut world: ResMut<WorldChanges>,
    mut pending: ResMut<PendingRestore>,
    mut commands: Commands,
    scene_entities: crate::DespawnOnTransition<'_, '_>,
    mut next_state: ResMut<NextState<crate::AppState>>,
    assets: Res<bevy::asset::AssetServer>,
    screenshot_target: Option<Res<crate::screenshot::ScreenshotTarget>>,
) {
    let Some(path) = requested.0.take() else {
        return;
    };
    let data = match read_save(&path) {
        Ok(data) => data,
        Err(e) => {
            crate::startup_log::log(&format!("load failed: {e}"));
            return;
        }
    };
    let Some(plugin) = plugin else {
        crate::startup_log::log("load failed: no parsed plugin");
        return;
    };
    crate::startup_log::log(&format!(
        "loading save: {} ({}, {})",
        path.display(),
        data.location,
        data.saved_at_display
    ));
    // Door memory + world changes FIRST: the spawner reads the
    // memory when it seeds the doors, and the world-changes
    // pass runs on scene entry.
    match door_memory.as_deref_mut() {
        Some(memory) => {
            memory.open = data.open_doors.iter().copied().collect();
        }
        None => {
            commands.insert_resource(DoorMemory {
                open: data.open_doors.iter().copied().collect(),
            });
        }
    }
    world.picked_up = data.picked_up.iter().copied().collect();
    world.looted_containers = data.looted_containers.iter().copied().collect();
    let target = save_target(&data);
    let spawn = save_spawn_point(&data);
    *pending = PendingRestore(Some(data));
    // Despawn the old scene and the player/camera; OnEnter(InGame)
    // spawns them fresh for the new scene.
    for entity in &scene_entities {
        commands.entity(entity).despawn();
    }
    // Load the save's cell on a worker thread from the shared
    // parsed plugin (no ESM re-parse, no BSA re-index) — the
    // door-transition path exactly.
    let title = format!("Loading {}...", target.display_name());
    let (tx, rx) = std::sync::mpsc::channel();
    let shared = plugin.0.clone();
    std::thread::spawn(move || {
        let result = crate::load_scene(&shared, &target, Some(spawn))
            .map(|data| crate::LoadResult::Scene { data });
        let _ = tx.send(result);
    });
    commands.insert_resource(crate::LoadReceiver(std::sync::Mutex::new(rx)));
    // No loading-screen UI on the screenshot path: there is no
    // window to show it on (its first load is synchronous and
    // shows nothing either). The windowed path gets the screen
    // with its own camera, like a door transition.
    if screenshot_target.is_none() {
        crate::spawn_loading_screen_ui(
            &mut commands,
            &assets,
            &title,
            "Loading your save...",
            true,
        );
    }
    next_state.set(crate::AppState::Loading);
}

/// Apply the pending restore on the first InGame frame after a
/// load: the inventory, stamina, settings, equipped spell, and
/// skill levels from the save, plus the exact saved view (the
/// spawn point already placed the player; this re-asserts the
/// transform and restores the pitch, which a spawn point
/// cannot express). Door memory + world changes were applied
/// at trigger time, not here. Clears the pending restore.
#[allow(clippy::too_many_arguments)]
fn apply_pending_restore(
    mut pending: ResMut<PendingRestore>,
    mut inventory: ResMut<Inventory>,
    mut stamina: Option<ResMut<Stamina>>,
    mut settings: ResMut<GameSettings>,
    mut character: Option<ResMut<CharacterState>>,
    mut skills: Option<ResMut<SkillIndex>>,
    mut player: Query<(&mut Transform, &mut LookAngles), With<Player>>,
) {
    let Some(data) = pending.0.take() else {
        return;
    };
    inventory.stacks = data
        .stacks
        .iter()
        .map(|(base, count)| InventoryStack {
            base_form_id: *base,
            count: *count,
        })
        .collect();
    inventory.gold = data.gold;
    inventory.equipped_weapon = data.equipped_weapon;
    inventory.equipped_armor = data
        .equipped_armor
        .iter()
        .filter_map(|(slot, base)| parse_equip_slot(slot).map(|s| (s, *base)))
        .collect();
    if let Some(stamina) = stamina.as_deref_mut() {
        stamina.value = data.stamina.clamp(0.0, crate::player::STAMINA_MAX);
    }
    settings.sensitivity = data.sensitivity;
    settings.invert_y = data.invert_y;
    if let Some(character) = character.as_deref_mut() {
        character.equipped_spell = data.equipped_spell;
    }
    if let Some(skills) = skills.as_deref_mut() {
        for (index, level) in &data.skill_levels {
            if let Some(entry) = skills.skills.iter_mut().find(|s| s.index == *index) {
                entry.level = *level;
            }
        }
    }
    if let Ok((mut transform, mut angles)) = player.single_mut() {
        transform.translation = Vec3::from(data.position);
        transform.rotation = Quat::from_euler(EulerRot::YXZ, data.yaw, data.pitch, 0.0);
        angles.yaw = data.yaw;
        angles.pitch = data.pitch;
    }
    crate::startup_log::log(&format!(
        "save restored: {} ({} stacks, {} gold, {} picked up, {} looted)",
        data.location,
        data.stacks.len(),
        data.gold,
        data.picked_up.len(),
        data.looted_containers.len()
    ));
}

/// Re-apply the world changes to the freshly built indexes.
/// Runs in the OnEnter(InGame) chain, AFTER
/// `build_interactables`: picked-up items leave the pickupable
/// index (and their entities leave the world), looted
/// containers leave the container index. Runs on every scene
/// entry — loads, door transitions, and the first load (a
/// no-op when nothing changed) — so a taken item stays taken
/// wherever the player goes.
pub fn apply_world_changes(
    mut commands: Commands,
    world: Res<WorldChanges>,
    mut pickupables: ResMut<Pickupables>,
    mut containers: ResMut<Containers>,
    scene_entities: Query<(Entity, &Transform), With<SceneEntity>>,
) {
    if world.picked_up.is_empty() && world.looted_containers.is_empty() {
        return;
    }
    let mut removed_items = 0;
    for ref_form_id in &world.picked_up {
        if let Some(item) = pickupables.items.remove(ref_form_id) {
            crate::interact::despawn_pickupable(&mut commands, &scene_entities, &item);
            removed_items += 1;
        }
    }
    let mut removed_containers = 0;
    for ref_form_id in &world.looted_containers {
        if containers.items.remove(ref_form_id).is_some() {
            removed_containers += 1;
        }
    }
    if removed_items > 0 || removed_containers > 0 {
        crate::startup_log::log(&format!(
            "world changes applied: {removed_items} picked-up items, {removed_containers} looted containers"
        ));
    }
}

/// The pause screen's E-on-Load: `pause_keys` sets
/// `PauseState::load_requested` (it is at Bevy's 16-parameter
/// limit and cannot take `LoadRequested` itself); this system
/// turns the flag into a `LoadRequested` for the selected save
/// and clears it.
fn apply_pause_load_request(
    mut state: ResMut<crate::pause::PauseState>,
    saves: Res<SaveList>,
    mut requested: ResMut<LoadRequested>,
) {
    if !state.load_requested {
        return;
    }
    state.load_requested = false;
    let selected = state.save.min(saves.saves.len().saturating_sub(1));
    if let Some(save) = saves.saves.get(selected) {
        requested.0 = Some(save.path.clone());
    }
}

/// The `--save-test` seed: write a save on the first InGame
/// frame, then consume the seed.
fn apply_save_seed(
    mut commands: Commands,
    seed: Option<Res<SaveSeed>>,
    mut requested: ResMut<SaveRequested>,
) {
    if seed.is_some() {
        requested.0 = true;
        commands.remove_resource::<SaveSeed>();
    }
}

/// The `--load-test <file>` seed: load the named save file
/// (a file name inside the save dir) on the first InGame
/// frame, then consume the seed.
fn apply_load_test_seed(
    mut commands: Commands,
    seed: Option<Res<LoadTestSeed>>,
    save_dir: Option<Res<SaveDir>>,
    mut requested: ResMut<LoadRequested>,
) {
    let (Some(seed), Some(dir)) = (seed, save_dir) else {
        return;
    };
    requested.0 = Some(dir.0.join(&seed.file));
    commands.remove_resource::<LoadTestSeed>();
}

pub struct SavePlugin;

impl Plugin for SavePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SaveList>()
            .init_resource::<SaveRequested>()
            .init_resource::<LoadRequested>()
            .init_resource::<PendingRestore>()
            .init_resource::<WorldChanges>()
            .add_systems(
                Update,
                (
                    apply_save_seed,
                    apply_load_test_seed,
                    apply_pause_load_request,
                    load_save_on_request,
                    apply_pending_restore,
                    write_save_on_request,
                )
                    .chain()
                    .run_if(bevy::prelude::in_state(crate::AppState::InGame)),
            );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dovah-save-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_data() -> SaveData {
        SaveData {
            version: SAVE_FORMAT_VERSION,
            cell_edid: "RiverwoodSleepingGiantInn".to_string(),
            cell_form_id: 0x0001_33c6,
            interior: true,
            exterior_grid: None,
            location: "Sleeping Giant Inn".to_string(),
            position: [1.0, 2.7, 3.0],
            yaw: 0.5,
            pitch: -0.1,
            health: 100.0,
            magicka: 100.0,
            stamina: 87.5,
            level: 1,
            gold: 42,
            stacks: vec![(0x0000_0f00, 3)],
            equipped_weapon: Some(0x0001_39b5),
            equipped_armor: vec![(String::from("Body"), 0x0001_39ed)],
            equipped_spell: Some(0x0001_2fcd),
            skill_levels: vec![(6, 20), (7, 25), (8, 15)],
            open_doors: vec![0x0001_3419],
            picked_up: vec![0x0006_00ee],
            looted_containers: vec![0x0006_00ff],
            game_date: "17th of Last Seed, 4E 201".to_string(),
            sensitivity: crate::pause::Sensitivity::OneHalf,
            invert_y: true,
            saved_at_unix: 1_794_749_400,
            saved_at_display: format_save_time(1_794_749_400),
        }
    }

    #[test]
    fn format_save_time_known_dates() {
        // Timestamps verified with `date -d @<unix> -u`.
        assert_eq!(format_save_time(1_794_749_400), "Nov 15, 2026, 1:30 PM UTC");
        assert_eq!(format_save_time(1_798_761_600), "Jan 1, 2027, 12:00 AM UTC");
        assert_eq!(
            format_save_time(1_772_323_140),
            "Feb 28, 2026, 11:59 PM UTC"
        );
    }

    #[test]
    fn save_round_trip_through_json_field_by_field() {
        let dir = test_dir("roundtrip");
        let data = test_data();
        let path = write_save(&dir, &data).expect("write_save");
        assert!(path.exists());
        let back = read_save(&path).expect("read_save");
        assert_eq!(back.version, SAVE_FORMAT_VERSION);
        assert_eq!(back.cell_edid, "RiverwoodSleepingGiantInn");
        assert_eq!(back.cell_form_id, 0x0001_33c6);
        assert!(back.interior);
        assert_eq!(back.exterior_grid, None);
        assert_eq!(back.location, "Sleeping Giant Inn");
        assert_eq!(back.position, [1.0, 2.7, 3.0]);
        assert!((back.yaw - 0.5).abs() < f32::EPSILON);
        assert!((back.pitch - (-0.1)).abs() < f32::EPSILON);
        assert!((back.health - 100.0).abs() < f32::EPSILON);
        assert!((back.magicka - 100.0).abs() < f32::EPSILON);
        assert!((back.stamina - 87.5).abs() < f32::EPSILON);
        assert_eq!(back.level, 1);
        assert_eq!(back.gold, 42);
        assert_eq!(back.stacks, vec![(0x0000_0f00, 3)]);
        assert_eq!(back.equipped_weapon, Some(0x0001_39b5));
        assert_eq!(
            back.equipped_armor,
            vec![(String::from("Body"), 0x0001_39ed)]
        );
        assert_eq!(back.equipped_spell, Some(0x0001_2fcd));
        assert_eq!(back.skill_levels, vec![(6, 20), (7, 25), (8, 15)]);
        assert_eq!(back.open_doors, vec![0x0001_3419]);
        assert_eq!(back.picked_up, vec![0x0006_00ee]);
        assert_eq!(back.looted_containers, vec![0x0006_00ff]);
        assert_eq!(back.game_date, "17th of Last Seed, 4E 201");
        assert!(back.invert_y);
        assert_eq!(back.saved_at_display, "Nov 15, 2026, 1:30 PM UTC");
        // And the list view (SaveInfo) sees the same save.
        let saves = list_saves(&dir);
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].location, "Sleeping Giant Inn");
        assert_eq!(saves[0].level, 1);
        assert_eq!(saves[0].game_date, "17th of Last Seed, 4E 201");
        assert_eq!(saves[0].saved_at_unix, 1_794_749_400);
        assert_eq!(saves[0].saved_at_display, "Nov 15, 2026, 1:30 PM UTC");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_saves_skips_garbage_and_wrong_versions() {
        let dir = test_dir("garbage");
        // A garbage file and a wrong-version save must not
        // break the list (or crash the panel).
        std::fs::write(dir.join("save-1.json"), b"not json at all").unwrap();
        std::fs::write(
            dir.join("save-2.json"),
            serde_json::json!({"version": 99, "cell_edid": "X"}).to_string(),
        )
        .unwrap();
        std::fs::write(dir.join("settings.json"), b"{}").unwrap();
        assert!(list_saves(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_version_saves_fail_cleanly() {
        // A v1 file (UI milestone 1 — no spells, skills,
        // doors, world changes) is rejected by version, not
        // loaded as v2 with invented state.
        let dir = test_dir("v1");
        let v1 = serde_json::json!({
            "version": 1,
            "cell_edid": "RiverwoodSleepingGiantInn",
            "cell_form_id": 0x133c6,
            "interior": true,
            "location": "Sleeping Giant Inn",
            "position": [1.0, 2.7, 3.0],
            "yaw": 0.5,
            "pitch": -0.1,
            "stamina": 87.5,
            "level": 1,
            "gold": 42,
            "stacks": [],
            "equipped_weapon": null,
            "equipped_armor": [],
            "sensitivity": "Normal",
            "invert_y": false,
            "saved_at_unix": 1_794_749_400,
            "saved_at_display": "Nov 15, 2026, 1:30 PM UTC",
        });
        let path = dir.join("save-1794749400.json");
        std::fs::write(&path, v1.to_string()).unwrap();
        let err = read_save(&path).expect_err("a v1 file must fail");
        assert!(err.contains("version 1"), "{err}");
        assert!(list_saves(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_saves_orders_newest_first() {
        let dir = test_dir("order");
        let mut a = test_data();
        a.saved_at_unix = 100;
        a.saved_at_display = format_save_time(100);
        let mut b = test_data();
        b.cell_edid = "B".to_string();
        b.location = "B".to_string();
        b.saved_at_unix = 200;
        b.saved_at_display = format_save_time(200);
        write_save(&dir, &a).unwrap();
        write_save(&dir, &b).unwrap();
        // A later write of A (a later unix second) sorts first.
        a.saved_at_unix = 300;
        a.saved_at_display = format_save_time(300);
        write_save(&dir, &a).unwrap();
        let saves = list_saves(&dir);
        assert_eq!(saves.len(), 3);
        assert_eq!(saves[0].saved_at_unix, 300);
        assert_eq!(saves[1].saved_at_unix, 200);
        assert_eq!(saves[2].saved_at_unix, 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn autosave_overwrites_one_slot() {
        let dir = test_dir("autosave");
        let mut a = test_data();
        a.saved_at_unix = 100;
        write_autosave(&dir, &a).unwrap();
        a.saved_at_unix = 200;
        a.gold = 99;
        write_autosave(&dir, &a).unwrap();
        // One file, the later write.
        let saves = list_saves(&dir);
        assert_eq!(saves.len(), 1);
        let back = read_save(&dir.join(AUTOSAVE_FILENAME)).unwrap();
        assert_eq!(back.saved_at_unix, 200);
        assert_eq!(back.gold, 99);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn yaw_flip_round_trips_through_the_spawn_view() {
        // A saved Bevy yaw b becomes the Skyrim yaw -b in the
        // spawn point; `spawn_viewpoint`'s forward from -b,
        // read back through `LookAngles::from_forward`, must be
        // b again — or the player loads facing the wrong way.
        // (The spawn math lives in scene.rs; this test pins the
        // conversion the save side owns.)
        for bevy_yaw in [0.0, 0.5, -0.842, 1.783, 3.125, -2.5] {
            let skyrim = bevy_yaw_to_skyrim_yaw(bevy_yaw);
            // spawn_viewpoint's forward, from the Skyrim yaw:
            // (sin s, cos s, 0) in Skyrim space -> Bevy.
            let forward_sky = [skyrim.sin(), skyrim.cos(), 0.0];
            let forward = coords::to_bevy_direction(forward_sky);
            let angles = LookAngles::from_forward(forward);
            assert!(
                (angles.yaw - bevy_yaw).abs() < 1e-5,
                "bevy yaw {bevy_yaw} -> skyrim {skyrim} -> back {}",
                angles.yaw
            );
        }
    }

    #[test]
    fn spawn_point_round_trips_the_saved_view() {
        // The eye position (Bevy metres) becomes a feet spawn
        // point (Skyrim units); converting back and adding the
        // eye height must restore the saved eye position.
        let data = test_data();
        let spawn = save_spawn_point(&data);
        let feet_bevy = coords::to_bevy_position(spawn.position);
        let eye = feet_bevy + Vec3::new(0.0, EYE_HEIGHT, 0.0);
        for i in 0..3 {
            assert!(
                (eye.to_array()[i] - data.position[i]).abs() < 0.01,
                "eye {eye:?} vs saved {:?}",
                data.position
            );
        }
        assert!((spawn.yaw - (-data.yaw)).abs() < f32::EPSILON);
    }

    #[test]
    fn exterior_grid_is_the_lands_median() {
        // The 3x3 Riverwood block's centre is (5, -11).
        let lands: Vec<LandData> = (4..=6)
            .flat_map(|gx| {
                (-12..=-10).map(move |gy| LandData {
                    grid: (gx, gy),
                    heights: [0.0; crate::land::LAND_SIZE * crate::land::LAND_SIZE],
                    colors: None,
                    normals: None,
                    base_ltex: None,
                    base_diffuse: None,
                    layers: Vec::new(),
                })
            })
            .collect();
        assert_eq!(exterior_center_grid(&lands), Some((5, -11)));
        assert_eq!(exterior_center_grid(&[]), None);
    }

    #[test]
    fn save_target_uses_edid_or_grid() {
        let mut data = test_data();
        assert!(matches!(
            save_target(&data),
            SceneTarget::Interior { edid, .. } if edid == "RiverwoodSleepingGiantInn"
        ));
        data.interior = false;
        data.exterior_grid = Some((5, -11));
        assert!(matches!(
            save_target(&data),
            SceneTarget::Exterior { grid: (5, -11) }
        ));
        // An exterior save without a grid falls back to
        // Riverwood rather than failing the load.
        data.exterior_grid = None;
        assert!(matches!(
            save_target(&data),
            SceneTarget::Exterior { grid: (5, -11) }
        ));
    }

    #[test]
    fn equip_slot_names_parse_back() {
        assert_eq!(parse_equip_slot("Head"), Some(EquipSlot::Head));
        assert_eq!(parse_equip_slot("Body"), Some(EquipSlot::Body));
        assert_eq!(parse_equip_slot("Hands"), Some(EquipSlot::Hands));
        assert_eq!(parse_equip_slot("Feet"), Some(EquipSlot::Feet));
        assert_eq!(parse_equip_slot("Shield"), Some(EquipSlot::Shield));
        assert_eq!(parse_equip_slot("Cloak"), None);
        // Every variant's Debug name parses back.
        for slot in [
            EquipSlot::Head,
            EquipSlot::Body,
            EquipSlot::Hands,
            EquipSlot::Feet,
            EquipSlot::Shield,
        ] {
            assert_eq!(parse_equip_slot(&format!("{slot:?}")), Some(slot));
        }
    }

    #[test]
    fn restore_applies_the_save_in_a_bare_app() {
        // The restore, end to end in a minimal app: a pending
        // save replaces the inventory, stamina, settings,
        // equipped spell, skill levels, and the player's
        // transform + look angles.
        use bevy::app::{App, Update};
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins);
        let data = test_data();
        app.insert_resource(PendingRestore(Some(data.clone())));
        app.insert_resource(Inventory::default());
        app.insert_resource(Stamina { value: 10.0 });
        app.insert_resource(GameSettings::default());
        app.insert_resource(CharacterState::default());
        app.insert_resource(SkillIndex {
            skills: vec![
                crate::character::SkillEntry {
                    index: 6,
                    name: "One-Handed".to_string(),
                    desc: String::new(),
                    level: 15,
                },
                crate::character::SkillEntry {
                    index: 7,
                    name: "Two-Handed".to_string(),
                    desc: String::new(),
                    level: 15,
                },
                crate::character::SkillEntry {
                    index: 8,
                    name: "Archery".to_string(),
                    desc: String::new(),
                    level: 15,
                },
            ],
            built: true,
            race_name: "Nord".to_string(),
        });
        app.world_mut().spawn((
            Transform::from_translation(Vec3::ZERO),
            LookAngles::default(),
            Player,
        ));
        app.add_systems(Update, apply_pending_restore);
        app.update();

        let inventory = app.world().resource::<Inventory>();
        assert_eq!(inventory.gold, 42);
        assert_eq!(inventory.count(0x0000_0f00), 3);
        assert_eq!(inventory.equipped_weapon, Some(0x0001_39b5));
        assert_eq!(
            inventory.equipped_armor.get(&EquipSlot::Body),
            Some(&0x0001_39ed)
        );
        let stamina = app.world().resource::<Stamina>();
        assert!((stamina.value - 87.5).abs() < f32::EPSILON);
        let settings = app.world().resource::<GameSettings>();
        assert_eq!(settings.sensitivity, crate::pause::Sensitivity::OneHalf);
        assert!(settings.invert_y);
        let character = app.world().resource::<CharacterState>();
        assert_eq!(character.equipped_spell, Some(0x0001_2fcd));
        let skills = app.world().resource::<SkillIndex>();
        assert_eq!(skills.get(6).unwrap().level, 20);
        assert_eq!(skills.get(7).unwrap().level, 25);
        assert_eq!(skills.get(8).unwrap().level, 15);
        let mut query = app.world_mut().query::<(&Transform, &LookAngles)>();
        let (transform, angles) = query.single(app.world()).unwrap();
        assert_eq!(transform.translation, Vec3::from(data.position));
        assert!((angles.yaw - 0.5).abs() < f32::EPSILON);
        assert!((angles.pitch - (-0.1)).abs() < f32::EPSILON);
        // The pending restore is consumed.
        assert!(app.world().resource::<PendingRestore>().0.is_none());
    }

    #[test]
    fn world_changes_remove_taken_items_from_the_fresh_indexes() {
        // The scene-entry pass: a picked-up tankard leaves the
        // pickupable index (and its entity leaves the world) and
        // a looted chest leaves the container index; untouched
        // items stay.
        use bevy::app::{App, Update};
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins);
        let mut world = WorldChanges::default();
        world.picked_up.insert(0x600ee);
        world.looted_containers.insert(0x700aa);
        app.insert_resource(world);
        app.insert_resource(Pickupables {
            items: [
                (
                    0x600ee,
                    crate::interact::Pickupable {
                        base_form_id: 0x300,
                        position: Vec3::new(1.0, 0.0, 0.0),
                        rotation: Quat::IDENTITY,
                        harvest_name: None,
                    },
                ),
                (
                    0x600ef,
                    crate::interact::Pickupable {
                        base_form_id: 0x301,
                        position: Vec3::new(2.0, 0.0, 0.0),
                        rotation: Quat::IDENTITY,
                        harvest_name: None,
                    },
                ),
            ]
            .into_iter()
            .collect(),
        });
        app.insert_resource(Containers {
            items: [
                (
                    0x700aa,
                    crate::interact::Container {
                        position: Vec3::new(3.0, 0.0, 0.0),
                        contents: vec![(0x400, 1)],
                    },
                ),
                (
                    0x700ab,
                    crate::interact::Container {
                        position: Vec3::new(4.0, 0.0, 0.0),
                        contents: vec![(0x401, 1)],
                    },
                ),
            ]
            .into_iter()
            .collect(),
        });
        app.world_mut().spawn((
            Transform::from_translation(Vec3::new(1.0, 0.0, 0.0)),
            SceneEntity,
        ));
        app.world_mut().spawn((
            Transform::from_translation(Vec3::new(2.0, 0.0, 0.0)),
            SceneEntity,
        ));
        app.add_systems(Update, apply_world_changes);
        app.update();

        let pickupables = app.world().resource::<Pickupables>();
        assert!(!pickupables.items.contains_key(&0x600ee));
        assert!(pickupables.items.contains_key(&0x600ef));
        let containers = app.world().resource::<Containers>();
        assert!(!containers.items.contains_key(&0x700aa));
        assert!(containers.items.contains_key(&0x700ab));
        // The taken tankard's entity is gone; the other's stays.
        let mut query = app.world_mut().query::<&Transform>();
        let positions: Vec<Vec3> = query.iter(app.world()).map(|t| t.translation).collect();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0], Vec3::new(2.0, 0.0, 0.0));
    }
}
