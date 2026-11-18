//! Dialogue (M18): talk to the inn's NPCs — Delphine, Orgnar,
//! Embry — with their REAL lines from the game's records.
//!
//! The data chain, all proven from Dylan's real files (see
//! `dovah_esm::dialogue` and the `dialogue_probe*` examples):
//!
//! * DIAL records are topics; `SNAM == "CUST"` is a
//!   player-selectable topic, `SNAM == "HELO"` a greeting. The
//!   topic text is the `FULL` field — a string ID into
//!   `.STRINGS` — and it is the PLAYER's line.
//! * INFO records (under each DIAL in a `TopicChildren` group)
//!   hold the responses. `NAM1` is a string ID into
//!   `.ILSTRINGS` — the NPC's line. One INFO can hold several
//!   NAM1s (alternatives); some INFOs are script-only and have
//!   no NAM1 at all.
//! * An INFO targets an NPC through a CTDA condition with
//!   function 72 (`GetIsID`), param1 = the NPC's form ID.
//!   Everything else about conditions (quest stages, factions)
//!   is NOT evaluated in M18: the topic list shows all of the
//!   NPC's topics, and that is stated plainly, not faked.
//!
//! The strings live in `Skyrim_-_Patch.bsa` at runtime
//! (`strings/skyrim_english.strings`, `...ilstrings`, ...),
//! read through the asset provider — no separate Strings folder
//! on the phone. `test-data/strings/` is the desktop-dev copy.
//!
//! USE priority (M18): door > dialogue > pickup > container.
//! `dialogue_use` runs after `door_use` and before
//! `pickup_use` in lib.rs's chained USE system, and the pickup
//! / container systems skip the press while dialogue is open
//! or an NPC is in dialogue range — so one press does one
//! thing. While dialogue is open, movement and mouse-look
//! freeze (the same `DialogueOpen` gate pattern as M17's
//! `InventoryOpen`).
//!
//! The menu IS the game's dialogue menu (from
//! dialoguemenu.swf, interface-extract/): the topic list on
//! the RIGHT over a fading panel (the list at (797.6, 359.7),
//! 8 rows at a 40 px pitch, the selected row fixed at the
//! 5th slot — the centered list), the speaker's name above
//! the subtitle at the bottom (name at (546.5, 435.9), the
//! line at (248.5, 576.7), 800 wide), and the [Tab] Goodbye
//! button at the bottom right. Up/Down move through the
//! topics, Tab/Esc close. The keyboard moves the selection
//! and the response follows at once (the game's separate
//! highlight-then-select is a documented simplification —
//! M18's state has one `selected` field, and clicking a topic
//! already shows its response immediately).
//!
//! The .swf's panel gradient is radial and its stops are not
//! decodable with the current tooling, so the panel is a
//! left-to-right strip fade (transparent to dark) — an
//! approximation, not the game's gradient. The topic list is
//! widened from the .swf's 350 px to the stage edge (482 px)
//! and the topic font is 18 px instead of the .swf's 24: the
//! default font is wider than the game's condensed one, and at
//! 24 px / 350 px half of Delphine's topics clip. The very
//! longest topic still clips a few characters of its tail —
//! the font limitation, not a layout bug.

use crate::assets::AssetProvider;
use crate::cell::CellData;
use crate::coords;
use crate::interact;
use crate::inventory::{
    centered_scroll_y, fs, px, InterfaceIcons, InventoryOpen, UiScale, STAGE_HEIGHT, STAGE_WIDTH,
};
use crate::scene::Player;
use crate::touch::TouchInputState;
use bevy::app::{App, Plugin, Update};
use bevy::color::Color;
use bevy::ecs::prelude::*;
use bevy::input::keyboard::KeyCode;
use bevy::input::ButtonInput;
use bevy::math::{Vec2, Vec3};
use bevy::picking::events::PointerClick;
use bevy::text::{Justify, TextColor, TextFont, TextLayout};
use bevy::transform::components::Transform;
use bevy::ui::widget::Text;
use bevy::ui::{
    AlignItems, BackgroundColor, FlexDirection, JustifyContent, Node, Overflow, PositionType,
    ScrollPosition, Val,
};
use dovah_esm::dialogue::{full_string_id, parse_dial, parse_info, topic_children};
use dovah_esm::{Plugin as EsmPlugin, Record};
use dovah_strings::{StringsFile, StringsKind, StringsSet};
use std::collections::HashMap;

/// The five plugins whose strings the engine loads, in load
/// order — the index IS the form-ID top byte (Skyrim.esm = 0,
/// Update.esm = 1, Dawnguard.esm = 2, HearthFires.esm = 3,
/// Dragonborn.esm = 4). The names are the BSA stored paths'
/// stems: `strings/<stem>_english.strings` /
/// `strings/<stem>_english.ilstrings` inside Patch.bsa.
const STRING_PLUGINS: [&str; 5] = ["skyrim", "update", "dawnguard", "hearthfires", "dragonborn"];

// ---------------------------------------------------------------------------
// The vanilla dialogue layout, from dialoguemenu.swf (stage 1280x720).
// The menu's origin is (599.5, 309.7); the constants below are ABSOLUTE
// stage coordinates, so the origin never needs applying at runtime.
// Pinned by `dialogue_layout_constants_match_the_swf`.
// ---------------------------------------------------------------------------

/// The topic panel's left edge (its ListPanel starts at x=148.1
/// rel. to the menu origin). The panel is 628 wide in the .swf —
/// it runs off the right edge of the stage, so the visible
/// width is 1280 - 747.6.
const TOPIC_PANEL_X: f32 = 747.6;
/// The topic panel's top edge (y=-220 rel. to the menu origin).
const TOPIC_PANEL_Y: f32 = 89.7;
/// The visible panel width: 1280 - 747.6.
const TOPIC_PANEL_W: f32 = 532.4;
/// The visible panel height: 720 - 89.7.
const TOPIC_PANEL_H: f32 = 630.3;
/// The topic list's left edge (the ListPanel's List_mc at x=50
/// rel. to the panel).
const TOPIC_LIST_X: f32 = 797.6;
/// The topic list's top edge (y=270 rel. to the panel).
const TOPIC_LIST_Y: f32 = 359.7;
/// The topic list's width — WIDENED from the .swf's 350 px to
/// the stage edge: the default font is wider than the game's
/// condensed one, and 350 px clips ordinary topics. The panel
/// runs to the right edge, so the extra width has room.
const TOPIC_LIST_W: f32 = 482.0;
/// A topic row's height (the .swf's 40 px pitch).
const TOPIC_ROW_H: f32 = 40.0;
/// The selected row's height (Entry4 at y=160, Entry5 at y=205
/// — the selected row is 45 px tall).
const TOPIC_ROW_SELECTED_H: f32 = 45.0;
/// The selected row's y in the centered list (Entry4's y=160).
const TOPIC_SELECTED_Y: f32 = 160.0;
/// The visible list height: 8 rows (the last starts at y=285).
const TOPIC_VIEWPORT_H: f32 = 325.0;
/// The topic text size — 18, not the .swf's 24: at 24 the
/// default font is so wide that half of Delphine's topics clip
/// at the list edge, and at 20 the two longest still clip.
/// 18 px fits all but the longest topic in the widened list
/// (the font limitation, documented in the module doc).
const TOPIC_FONT_SIZE: f32 = 18.0;
/// The subtitle's left edge (x=-351 rel. to the menu origin).
const SUBTITLE_X: f32 = 248.5;
/// The subtitle's top edge (y=267 rel. to the menu origin).
const SUBTITLE_Y: f32 = 576.7;
/// The subtitle's width (800 in the .swf).
const SUBTITLE_W: f32 = 800.0;
/// The subtitle's height (61 in the .swf — two lines at 22 px).
const SUBTITLE_H: f32 = 61.0;
/// The subtitle's text size (the .swf's size 22).
const SUBTITLE_FONT_SIZE: f32 = 22.0;
/// The speaker name's left edge (x=-53 rel. to the menu origin).
const SPEAKER_X: f32 = 546.5;
/// The speaker name's top edge (y=126.2 rel. to the menu
/// origin).
const SPEAKER_Y: f32 = 435.9;
/// The speaker name's width (200 in the .swf).
const SPEAKER_W: f32 = 200.0;
/// The speaker name's text size (the .swf's size 22).
const SPEAKER_FONT_SIZE: f32 = 22.0;
/// The exit button's left edge (x=345 rel. to the menu origin).
const EXIT_BUTTON_X: f32 = 944.5;
/// The exit button's lowest y: just below the 8th topic row
/// (the .swf's 669.7 overlaps the 8th row by 7 px, so the
/// button sits 10 px lower when the list is full). With fewer
/// topics the button rises to sit just below the last row.
const EXIT_BUTTON_MAX_Y: f32 = 680.0;
/// The gap between the last visible topic row and the exit
/// button.
const EXIT_BUTTON_GAP: f32 = 8.0;

/// The exit button's y for a list showing `visible_rows` rows:
/// just below the last row, capped at `EXIT_BUTTON_MAX_Y`.
fn exit_button_y(visible_rows: usize) -> f32 {
    let below_last = TOPIC_LIST_Y + visible_rows as f32 * TOPIC_ROW_H + EXIT_BUTTON_GAP;
    below_last.min(EXIT_BUTTON_MAX_Y)
}

/// The number of topic rows visible at once (the .swf's
/// template holds 8).
const TOPIC_VISIBLE_ROWS: usize = 8;

/// All five plugins' strings, loaded once at startup. A record's
/// text resolves in the set of ITS plugin (the form ID's top
/// byte) — the same string ID means different text in different
/// plugins (NAM1 10739 is "There you are." in Skyrim and "I
/// think you may have wasted your money, friend." in
/// Dragonborn), so the set must never be guessed from the ID.
#[derive(Debug, Resource)]
pub struct GameStrings {
    pub sets: Vec<StringsSet>,
}

impl Default for GameStrings {
    /// One empty set (the Skyrim slot), so `set_for` never
    /// panics before the real strings load — lookups just miss
    /// and callers fall back to prettified EDIDs.
    fn default() -> Self {
        GameStrings {
            sets: vec![StringsSet::default()],
        }
    }
}

impl GameStrings {
    /// Load every plugin's `.STRINGS` + `.ILSTRINGS` +
    /// `.DLSTRINGS` from the asset provider (Patch.bsa). A
    /// missing or unparsable file degrades to an empty set for
    /// that plugin — names fall back to prettified EDIDs (M17
    /// behaviour) instead of failing the load. The long-form
    /// file (inventory redesign) holds book descriptions (BOOK
    /// `CNAM`).
    pub fn load(provider: &mut AssetProvider) -> Self {
        let mut sets = Vec::with_capacity(STRING_PLUGINS.len());
        for stem in STRING_PLUGINS {
            let general = provider
                .read_file(&format!("strings/{stem}_english.strings"))
                .and_then(|bytes| StringsFile::from_bytes(&bytes, StringsKind::General).ok())
                .unwrap_or_default();
            let dialogue = provider
                .read_file(&format!("strings/{stem}_english.ilstrings"))
                .and_then(|bytes| StringsFile::from_bytes(&bytes, StringsKind::LengthPrefixed).ok())
                .unwrap_or_default();
            let longform = provider
                .read_file(&format!("strings/{stem}_english.dlstrings"))
                .and_then(|bytes| StringsFile::from_bytes(&bytes, StringsKind::LengthPrefixed).ok())
                .unwrap_or_default();
            crate::startup_log::log(&format!(
                "strings loaded: {stem} ({} general, {} dialogue, {} long-form)",
                general.len(),
                dialogue.len(),
                longform.len()
            ));
            sets.push(StringsSet {
                general,
                dialogue,
                longform,
            });
        }
        GameStrings { sets }
    }

    /// The strings set for a form ID's plugin (top byte). An
    /// out-of-range index (a form ID from a plugin we do not
    /// load) falls back to the Skyrim set — the safest guess,
    /// and the set the inn's records live in. The default holds
    /// one empty set, so this never panics.
    pub fn set_for(&self, form_id: u32) -> &StringsSet {
        let index = (form_id >> 24) as usize;
        self.sets.get(index).unwrap_or(&self.sets[0])
    }

    /// A record's display name: its `FULL` field as a string ID,
    /// resolved in the record's own plugin set. `None` when the
    /// record has no 4-byte FULL field or the ID does not
    /// resolve — callers fall back to the prettified EDID.
    pub fn name_of(&self, record: &Record) -> Option<String> {
        let id = full_string_id(record)?;
        self.set_for(record.form_id)
            .general(id)
            .map(|s| s.to_string())
    }

    /// A book's description: its `CNAM` field as a string ID,
    /// resolved in the record's own plugin set's `.DLSTRINGS`
    /// file (book_desc_probe: 33/33 nonzero BOOK CNAMs in
    /// Skyrim.esm resolve there — mostly Atronach Forge recipe
    /// books, whose CNAM names the crafted result). `None` when
    /// the record has no 4-byte CNAM, the ID is 0 (most books
    /// carry no description), or the ID does not resolve.
    pub fn description_of(&self, record: &Record) -> Option<String> {
        let field = record.field(b"CNAM")?;
        if field.data.len() != 4 {
            return None;
        }
        let id = u32::from_le_bytes(field.data[0..4].try_into().unwrap());
        if id == 0 {
            return None;
        }
        self.set_for(record.form_id)
            .longform(id)
            .map(|s| s.to_string())
    }
}

/// One player-selectable topic for one NPC: the player's line
/// and the NPC's response, both real text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogueTopic {
    /// The DIAL form ID (for debugging / future quest gating).
    pub dial_form_id: u32,
    /// The player's line (DIAL FULL -> `.STRINGS`).
    pub text: String,
    /// The NPC's response (first resolvable INFO NAM1 ->
    /// `.ILSTRINGS`, first targeting INFO in file order).
    pub response: String,
}

/// The dialogue index: every NPC's topics, greetings, and real
/// name, built once per app run from the parsed plugin and the
/// strings. Topics keep DIAL file order (the game's priority
/// order). Topics with no speakable response — every targeting
/// INFO is script-only or unresolvable — are dropped, not
/// shown empty.
#[derive(Debug, Default, Resource)]
pub struct DialogueIndex {
    pub topics: HashMap<u32, Vec<DialogueTopic>>,
    pub greetings: HashMap<u32, String>,
    pub names: HashMap<u32, String>,
    pub built: bool,
}

impl DialogueIndex {
    pub fn build(plugin: &EsmPlugin, strings: &GameStrings) -> Self {
        let mut index = DialogueIndex::default();
        // Names first: every NPC_ record with a FULL resolves
        // its real name. (The inn's three are Delphine, Orgnar,
        // Embry — but the pass is one linear scan, so all NPCs
        // get names for the exteriors too.)
        for record in plugin.all_records() {
            if &record.record_type == b"NPC_" {
                if let Some(name) = strings.name_of(record) {
                    index.names.insert(record.form_id, name);
                }
            }
        }
        let children = topic_children(plugin);
        for dial_record in plugin.all_records() {
            let Some(dial) = parse_dial(dial_record) else {
                continue;
            };
            let Some(infos) = children.get(&dial.form_id) else {
                continue;
            };
            // The NPCs this topic's INFOs target (GetIsID).
            // One topic can target several NPCs (shared lines);
            // each gets the topic with the response from the
            // INFOs that target THEM.
            let mut targets: Vec<u32> = Vec::new();
            for info_record in infos {
                if let Some(info) = parse_info(info_record) {
                    for target in info.targets {
                        if !targets.contains(&target) {
                            targets.push(target);
                        }
                    }
                }
            }
            if targets.is_empty() {
                continue;
            }
            if dial.is_player_topic() {
                let Some(full_id) = dial.full_id else {
                    continue;
                };
                let Some(text) = strings
                    .set_for(dial_record.form_id)
                    .general(full_id)
                    .map(|s| s.to_string())
                else {
                    continue;
                };
                for target in targets {
                    let Some(response) = first_response_for(infos, target, strings) else {
                        continue;
                    };
                    index.topics.entry(target).or_default().push(DialogueTopic {
                        dial_form_id: dial.form_id,
                        text: text.clone(),
                        response,
                    });
                }
            } else if dial.is_greeting() {
                for target in targets {
                    // First HELO in file order wins — several
                    // of Delphine's greetings are late-quest
                    // lines, and without condition evaluation
                    // the file order is the only honest pick.
                    if index.greetings.contains_key(&target) {
                        continue;
                    }
                    if let Some(response) = first_response_for(infos, target, strings) {
                        index.greetings.insert(target, response);
                    }
                }
            }
        }
        index.built = true;
        index
    }
}

/// The first resolvable response line for one NPC under one
/// topic: the first targeting INFO in file order whose first
/// resolvable NAM1 (in file order) has text in the INFO's own
/// plugin set. `None` when every targeting INFO is script-only
/// or unresolvable.
fn first_response_for(infos: &[&Record], npc: u32, strings: &GameStrings) -> Option<String> {
    for info_record in infos {
        let Some(info) = parse_info(info_record) else {
            continue;
        };
        if !info.targets.contains(&npc) {
            continue;
        }
        for nam1 in &info.nam1_ids {
            if let Some(line) = strings
                .set_for(info_record.form_id)
                .dialogue(*nam1)
                .map(|s| s.to_string())
            {
                return Some(line);
            }
        }
    }
    None
}

/// One talkable NPC in the current scene (an ACHR ref).
#[derive(Debug, Clone)]
pub struct TalkableNpc {
    /// The NPC_ base form ID (the dialogue index key).
    pub base_form_id: u32,
    /// Bevy-space position (metres) — the USE range check.
    pub position: Vec3,
    /// Base editor ID — the name fallback when the NPC_ record
    /// has no FULL (prettified, M17 style).
    pub base_edid: String,
}

/// Talkable NPCs in the current scene, by ref form ID. Rebuilt
/// by `build_talkable_npcs` on every scene load.
#[derive(Debug, Default, Resource)]
pub struct TalkableNpcs {
    pub npcs: HashMap<u32, TalkableNpc>,
}

/// Whether a dialogue is open. Movement and mouse-look freeze
/// while it is (the same gate pattern as `InventoryOpen`).
#[derive(Debug, Default, Resource)]
pub struct DialogueOpen(pub bool);

/// The open conversation. Empty (`npc_ref: None`) when no
/// dialogue is open.
#[derive(Debug, Default, Resource)]
pub struct DialogueState {
    /// The ref form ID of the NPC being talked to (the state
    /// knows who the conversation is with, even though the UI
    /// only needs the name — the log and any future quest
    /// gating read it from here, not the UI).
    #[allow(dead_code)]
    pub npc_ref: Option<u32>,
    pub npc_name: String,
    pub greeting: String,
    pub topics: Vec<DialogueTopic>,
    /// The selected topic's index; `None` = showing the
    /// greeting.
    pub selected: Option<usize>,
}

impl DialogueState {
    /// The line currently shown: the selected topic's response,
    /// or the greeting when nothing is selected.
    pub fn current_line(&self) -> &str {
        match self.selected {
            Some(i) => self
                .topics
                .get(i)
                .map(|t| t.response.as_str())
                .unwrap_or(&self.greeting),
            None => &self.greeting,
        }
    }
}

/// Screenshot-proof seed (`--dialogue`): open dialogue with
/// the given NPC base form ID when present in the scene (else
/// the first talkable NPC), and select the first topic so the
/// shot shows a real response line. Inserted only on the
/// screenshot path; absent everywhere else.
#[derive(Debug, Resource)]
pub struct DialogueSeed {
    pub npc_base: Option<u32>,
    pub select_first_topic: bool,
}

/// Marker for the dialogue UI root (despawned on close).
#[derive(Component)]
pub struct DialogueUiRoot;

/// Marker for one topic row in the list.
#[derive(Component)]
struct DialogueTopicRow {
    index: usize,
}

/// Marker for the Goodbye button.
#[derive(Component)]
struct DialogueGoodbye;

pub struct DialoguePlugin;

impl Plugin for DialoguePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameStrings>()
            .init_resource::<DialogueIndex>()
            .init_resource::<TalkableNpcs>()
            .init_resource::<DialogueOpen>()
            .init_resource::<DialogueState>()
            .add_systems(
                Update,
                (close_dialogue_on_key, dialogue_keys, refresh_dialogue_ui),
            );
    }
}

/// Build the dialogue index (once per app run) from the parsed
/// plugin and the loaded strings. Runs in the scene-load chain,
/// after the plugin resource exists; the `built` flag makes
/// re-runs on door transitions no-ops.
pub fn build_dialogue_index(
    plugin: Option<Res<crate::SharedPlugin>>,
    strings: Option<Res<GameStrings>>,
    mut index: ResMut<DialogueIndex>,
) {
    let (Some(plugin), Some(strings)) = (plugin, strings) else {
        return;
    };
    if index.built {
        return;
    }
    let started = std::time::Instant::now();
    *index = DialogueIndex::build(&plugin.0, &strings);
    let topic_count: usize = index.topics.values().map(|v| v.len()).sum();
    crate::startup_log::log(&format!(
        "dialogue index built: {} NPCs with topics ({} topics), {} greetings, {} names ({} ms)",
        index.topics.len(),
        topic_count,
        index.greetings.len(),
        index.names.len(),
        started.elapsed().as_millis()
    ));
}

/// Build the talkable-NPC index for the loaded scene: every
/// ACHR ref that is a person — plus the rare creature that has
/// dialogue of its own. Creatures (chicken, cow, horse: NPC_
/// records with a creature race) are NOT talkable in the game,
/// and letting them in is the chicken dialogue bug: Dylan
/// walked up to a man in Riverwood, the nearest actor in USE
/// range was the chicken ref 0x001066de (12 m from the inn
/// door), and dialogue opened as "Chicken" with 0 topics. The
/// exception is a creature WITH dialogue — Paarthurnax is a
/// dragon and he talks — so a creature with a greeting or
/// topics in the index stays talkable. The dialogue index is
/// built earlier in the same OnEnter chain, so it is available
/// here; without it (or before it builds) the race rule alone
/// applies.
pub fn build_talkable_npcs(
    cell: Option<Res<CellData>>,
    index: Option<Res<DialogueIndex>>,
    mut talkable: ResMut<TalkableNpcs>,
) {
    let Some(cell) = cell else {
        return;
    };
    talkable.npcs.clear();
    for object in &cell.objects {
        if !object.is_actor {
            continue;
        }
        if object.is_creature && !has_dialogue(index.as_deref(), object.base_form_id) {
            continue;
        }
        talkable.npcs.insert(
            object.ref_form_id,
            TalkableNpc {
                base_form_id: object.base_form_id,
                position: coords::to_bevy_position(object.position),
                base_edid: object.base_edid.clone(),
            },
        );
    }
}

/// Whether an NPC_ base has any dialogue in the index — a
/// greeting or at least one topic. False when the index is
/// absent or has no entry for the base.
fn has_dialogue(index: Option<&DialogueIndex>, base_form_id: u32) -> bool {
    let Some(index) = index else {
        return false;
    };
    index.topics.contains_key(&base_form_id) || index.greetings.contains_key(&base_form_id)
}

/// The USE facing cone: a target counts as faced when it lies
/// within ±45° of the player's forward (the melee arc's
/// calibration — the same tolerance the combat hit rule uses).
/// Cos(45°); the dot-product threshold in `is_facing_target`.
pub const USE_FACING_COS: f32 = 0.70710677;

/// Whether `target` lies in the player's forward cone (see
/// `USE_FACING_COS`). Horizontal only — the vertical is the
/// range rule's job (`interact::in_use_range`). A target at the
/// player's feet (no horizontal direction) counts as faced, as
/// does a straight-up/down look (no horizontal forward). Pure —
/// unit-tested.
pub fn is_facing_target(feet: Vec3, forward: Vec3, target: Vec3) -> bool {
    let to = Vec3::new(target.x - feet.x, 0.0, target.z - feet.z);
    if to.length_squared() < 1e-6 {
        return true;
    }
    let fwd = Vec3::new(forward.x, 0.0, forward.z);
    if fwd.length_squared() < 1e-6 {
        return true;
    }
    to.normalize().dot(fwd.normalize()) >= USE_FACING_COS
}

/// The nearest talkable NPC in USE range of the player's feet
/// (same range rule as items — see `interact::in_use_range`),
/// by ref form ID. `None` when no NPC is close enough.
pub fn nearest_npc_in_range(talkable: &TalkableNpcs, feet: Vec3) -> Option<u32> {
    nearest_npc_filtered(talkable, feet, None)
}

/// The nearest talkable NPC in USE range AND in the player's
/// forward cone (v0.4.13: Embry was talkable from across the
/// room while Dylan looked at a chair — proximity alone is not
/// a target). `None` when no NPC is both close and faced.
pub fn nearest_npc_in_range_facing(
    talkable: &TalkableNpcs,
    feet: Vec3,
    forward: Vec3,
) -> Option<u32> {
    nearest_npc_filtered(talkable, feet, Some(forward))
}

fn nearest_npc_filtered(
    talkable: &TalkableNpcs,
    feet: Vec3,
    forward: Option<Vec3>,
) -> Option<u32> {
    let mut best: Option<(u32, f32)> = None;
    for (ref_form_id, npc) in &talkable.npcs {
        if !interact::in_use_range(npc.position, feet) {
            continue;
        }
        if let Some(fwd) = forward {
            if !is_facing_target(feet, fwd, npc.position) {
                continue;
            }
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

/// USE on an NPC opens dialogue. Runs in lib.rs's chained USE
/// system after `door_use` and before `pickup_use` — dialogue
/// outranks pickup and containers, doors outrank dialogue.
/// The NPC must be in range AND faced (v0.4.13: proximity
/// alone let Dylan talk to Embry while looking at a chair).
#[allow(clippy::too_many_arguments)]
pub fn dialogue_use(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    touch: Option<Res<TouchInputState>>,
    player: Query<&Transform, With<Player>>,
    talkable: Option<Res<TalkableNpcs>>,
    index: Option<Res<DialogueIndex>>,
    mut open: ResMut<DialogueOpen>,
    mut state: ResMut<DialogueState>,
    inventory_open: Option<Res<InventoryOpen>>,
    scale: Res<UiScale>,
    icons: Res<InterfaceIcons>,
    mut press_log: Option<ResMut<crate::touch::PressLog>>,
) {
    if open.0 || inventory_open.map(|o| o.0).unwrap_or(false) {
        return;
    }
    let use_pressed = keyboard.just_pressed(KeyCode::KeyE)
        || touch.as_deref().map(|t| t.use_pressed).unwrap_or(false);
    if !use_pressed {
        return;
    }
    let (Some(talkable), Some(index)) = (talkable, index) else {
        return;
    };
    let Ok(transform) = player.single() else {
        return;
    };
    let feet = Vec3::new(
        transform.translation.x,
        transform.translation.y - crate::player::EYE_HEIGHT,
        transform.translation.z,
    );
    let forward = *transform.forward();
    let Some(npc_ref) = nearest_npc_in_range_facing(&talkable, feet, forward) else {
        return;
    };
    open_dialogue(
        &mut commands,
        &index,
        &talkable,
        npc_ref,
        &mut open,
        &mut state,
        false,
        scale.0,
        &icons,
    );
    // Press log: `press: use #N -> talked to Embry` (a
    // keyboard-triggered open finds no pending press and logs
    // nothing — touch buttons only).
    if let Some(log) = press_log.as_deref_mut() {
        if let Some(count) = log.claim(crate::touch::PressButton::Use) {
            crate::startup_log::log(&crate::touch::PressLog::format_line(
                crate::touch::PressButton::Use,
                count,
                Some(&format!("talked to {}", state.npc_name)),
            ));
        }
    }
}

/// Open dialogue with one NPC: fill the state (name, greeting,
/// topics) and spawn the UI. Shared by `dialogue_use` and the
/// screenshot seed. `select_first` pre-selects the first topic
/// (the seed's proof shot shows a real response line, not just
/// the greeting) — the selection is set BEFORE the UI spawns,
/// so the first frame already shows the response.
#[allow(clippy::too_many_arguments)]
fn open_dialogue(
    commands: &mut Commands,
    index: &DialogueIndex,
    talkable: &TalkableNpcs,
    npc_ref: u32,
    open: &mut DialogueOpen,
    state: &mut DialogueState,
    select_first: bool,
    scale: f32,
    icons: &InterfaceIcons,
) {
    let Some(npc) = talkable.npcs.get(&npc_ref) else {
        return;
    };
    let name = index
        .names
        .get(&npc.base_form_id)
        .cloned()
        .unwrap_or_else(|| crate::inventory::prettify_edid(&npc.base_edid));
    let greeting = index
        .greetings
        .get(&npc.base_form_id)
        .cloned()
        .unwrap_or_default();
    let topics = index
        .topics
        .get(&npc.base_form_id)
        .cloned()
        .unwrap_or_default();
    let selected = if select_first && !topics.is_empty() {
        Some(0)
    } else {
        None
    };
    *state = DialogueState {
        npc_ref: Some(npc_ref),
        npc_name: name.clone(),
        greeting,
        topics,
        selected,
    };
    open.0 = true;
    spawn_dialogue_ui(commands, state, scale, icons);
    // The ref form ID is in the log line: the v0.4.4 chicken
    // bug ("dialogue opened: Chicken (0 topics)") could not be
    // traced to its ref because the line carried no ID.
    crate::startup_log::log(&format!(
        "dialogue opened: {name} ({} topics, ref 0x{npc_ref:08x})",
        state.topics.len()
    ));
}

/// Close the dialogue: despawn the UI, clear the state.
fn close_dialogue(
    commands: &mut Commands,
    ui_roots: &Query<Entity, With<DialogueUiRoot>>,
    open: &mut DialogueOpen,
    state: &mut DialogueState,
) {
    for entity in ui_roots.iter() {
        commands.entity(entity).despawn();
    }
    open.0 = false;
    *state = DialogueState::default();
}

/// Tab or Esc closes an open dialogue (keyboard). The touch
/// path closes via the Goodbye button.
fn close_dialogue_on_key(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut open: ResMut<DialogueOpen>,
    mut state: ResMut<DialogueState>,
    ui_roots: Query<Entity, With<DialogueUiRoot>>,
) {
    if !open.0 || !(keyboard.just_pressed(KeyCode::Escape) || keyboard.just_pressed(KeyCode::Tab)) {
        return;
    }
    close_dialogue(&mut commands, &ui_roots, &mut open, &mut state);
}

/// Up/Down move through the topics (wrapping). The response
/// follows the selection at once — the same as clicking a
/// topic (see the module doc on the game's separate
/// highlight-then-select).
fn dialogue_keys(
    keyboard: Res<ButtonInput<KeyCode>>,
    open: Res<DialogueOpen>,
    mut state: ResMut<DialogueState>,
) {
    if !open.0 || state.topics.is_empty() {
        return;
    }
    let count = state.topics.len();
    let current = state.selected;
    if keyboard.just_pressed(KeyCode::ArrowDown) {
        state.selected = Some(match current {
            Some(i) => (i + 1) % count,
            None => 0,
        });
    } else if keyboard.just_pressed(KeyCode::ArrowUp) {
        state.selected = Some(match current {
            Some(i) => (i + count - 1) % count,
            None => count - 1,
        });
    }
}

/// Rebuild the dialogue UI when the conversation state changed
/// (a topic was selected) or the interface icons finished
/// loading (the Goodbye button's Tab icon). The whole panel is
/// rebuilt — a few dozen nodes, cheaper than diffing rows —
/// like `refresh_inventory_ui`.
fn refresh_dialogue_ui(
    mut commands: Commands,
    open: Res<DialogueOpen>,
    state: Res<DialogueState>,
    scale: Option<Res<UiScale>>,
    icons: Option<Res<InterfaceIcons>>,
    ui_roots: Query<Entity, With<DialogueUiRoot>>,
) {
    // UiScale / InterfaceIcons live in the inventory plugin —
    // optional here so the dialogue plugin still runs in a
    // minimal app without it (the M17 lesson: a required Res
    // panics on the first tick when the resource doesn't exist
    // yet).
    let icons_changed = icons.as_ref().map(|i| i.is_changed()).unwrap_or(false);
    if !open.0 || !(state.is_changed() || icons_changed) {
        return;
    }
    let scale = scale.map(|s| s.0).unwrap_or(1.0);
    let icons = icons
        .as_deref()
        .map(|i| i as &InterfaceIcons)
        .unwrap_or(&DEFAULT_ICONS);
    for entity in ui_roots.iter() {
        commands.entity(entity).despawn();
    }
    spawn_dialogue_ui(&mut commands, &state, scale, icons);
}

/// The empty icon set, used when the inventory plugin (which
/// owns `InterfaceIcons`) is absent — the buttons fall back to
/// text.
static DEFAULT_ICONS: InterfaceIcons = InterfaceIcons {
    e: None,
    r: None,
    tab: None,
    loaded: false,
};

/// Apply the screenshot-proof seed (`--dialogue`): open
/// dialogue with the seeded NPC (or the first talkable NPC in
/// the scene) and select the first topic so the shot shows a
/// real response line. No-op without the seed resource. Runs in
/// the scene-load chain, after `build_dialogue_index` and
/// `build_talkable_npcs`.
#[allow(clippy::too_many_arguments)]
pub fn apply_dialogue_seed(
    mut commands: Commands,
    seed: Option<Res<DialogueSeed>>,
    talkable: Option<Res<TalkableNpcs>>,
    index: Option<Res<DialogueIndex>>,
    mut open: ResMut<DialogueOpen>,
    mut state: ResMut<DialogueState>,
    scale: Res<UiScale>,
    icons: Res<InterfaceIcons>,
) {
    let Some(seed) = seed else {
        return;
    };
    let (Some(talkable), Some(index)) = (talkable, index) else {
        commands.remove_resource::<DialogueSeed>();
        return;
    };
    // The seeded NPC by base form ID; else the first talkable
    // NPC in the scene (lowest ref form ID, for determinism).
    let npc_ref = seed
        .npc_base
        .and_then(|base| {
            talkable
                .npcs
                .iter()
                .find(|(_, npc)| npc.base_form_id == base)
                .map(|(ref_form_id, _)| *ref_form_id)
        })
        .or_else(|| talkable.npcs.keys().min().copied());
    if let Some(npc_ref) = npc_ref {
        open_dialogue(
            &mut commands,
            &index,
            &talkable,
            npc_ref,
            &mut open,
            &mut state,
            seed.select_first_topic,
            scale.0,
            &icons,
        );
    }
    commands.remove_resource::<DialogueSeed>();
}

/// Spawn the vanilla dialogue menu: the topic list on the
/// right over a fading panel, the speaker's name above the
/// subtitle at the bottom, the [Tab] Goodbye button at the
/// bottom right. All positions are the dialoguemenu.swf
/// stage coordinates, scaled by `scale` (window height / 720).
fn spawn_dialogue_ui(
    commands: &mut Commands,
    state: &DialogueState,
    scale: f32,
    _icons: &InterfaceIcons,
) {
    let topics = state.topics.clone();
    let selected = state.selected;
    let name = state.npc_name.clone();
    let line = state.current_line().to_string();
    let topic_count = topics.len();
    // The centered scroll: the selected row sits at the 5th
    // slot (y=160) while the list can scroll, clamped at both
    // ends — the same pattern as the inventory's item list.
    let content_h = topic_count as f32 * TOPIC_ROW_H
        + if topic_count > 0 {
            TOPIC_ROW_SELECTED_H - TOPIC_ROW_H
        } else {
            0.0
        };
    let selected_top = selected.unwrap_or(0) as f32 * TOPIC_ROW_H;
    let scroll_y = centered_scroll_y(selected_top, TOPIC_SELECTED_Y, content_h, TOPIC_VIEWPORT_H);
    let visible_rows = topic_count.min(TOPIC_VISIBLE_ROWS);
    let button_y = exit_button_y(visible_rows);
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..Default::default()
            },
            DialogueUiRoot,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: px(scale, STAGE_WIDTH),
                    height: px(scale, STAGE_HEIGHT),
                    ..Default::default()
                },
                DialogueUiRoot,
            ))
            .with_children(|stage| {
                // The speaker's name and the subtitle go down
                // FIRST: a long subtitle's centered lines run
                // under the topic list (the .swf's 800 px field
                // reaches x=1048.5), and the panel + list must
                // draw over the tail, as in the game — the
                // subtitle spawning last covered the topic rows
                // and made them unreadable.
                // The speaker's name, gold, centered above the
                // subtitle (the name field is centered on the
                // subtitle's center in the .swf).
                stage.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(scale, SPEAKER_X),
                        top: px(scale, SPEAKER_Y),
                        width: px(scale, SPEAKER_W),
                        ..Default::default()
                    },
                    Text::new(name),
                    TextFont {
                        font_size: fs(scale, SPEAKER_FONT_SIZE),
                        ..Default::default()
                    },
                    TextColor(Color::srgb(0.85, 0.7, 0.35)),
                    TextLayout::justify(Justify::Center),
                ));
                // The current line (greeting or selected
                // response), wrapped across the subtitle field
                // and CENTERED. The field is 800 px wide and
                // reaches under the topic list — so the text
                // sits in a clip parent that ends at the
                // list's left edge: a long line's tail is cut
                // there instead of interleaving with the topic
                // rows (the game hides the tail under its
                // gradient; the clip is the same look).
                stage
                    .spawn((Node {
                        position_type: PositionType::Absolute,
                        left: px(scale, SUBTITLE_X),
                        top: px(scale, SUBTITLE_Y),
                        width: px(scale, TOPIC_LIST_X - SUBTITLE_X),
                        height: px(scale, SUBTITLE_H),
                        overflow: Overflow::clip(),
                        ..Default::default()
                    },))
                    .with_children(|clip| {
                        clip.spawn((
                            Node {
                                // Relative (NOT absolute): the
                                // absolutely-positioned text
                                // escaped the clip parent on
                                // the phone — the tail rendered
                                // ~125 stage px past the clip
                                // edge, interleaving with the
                                // topic rows (v0.4.13 shot 04).
                                // flex_shrink 0 keeps the 800
                                // px width (a flex item would
                                // otherwise shrink to the
                                // parent's 549 px and re-wrap
                                // to three lines).
                                width: px(scale, SUBTITLE_W),
                                height: px(scale, SUBTITLE_H),
                                flex_shrink: 0.0,
                                ..Default::default()
                            },
                            Text::new(line),
                            TextFont {
                                font_size: fs(scale, SUBTITLE_FONT_SIZE),
                                ..Default::default()
                            },
                            TextColor(Color::WHITE),
                            TextLayout::justify(Justify::Center),
                        ));
                    });
                // The topic panel: the .swf's ListPanel is a
                // radial gradient whose stops are not
                // decodable, so ten vertical strips fade from
                // transparent on the left to dark on the right
                // — the same soft wash the game shows behind
                // the topics. The fade runs to 0.75 so a long
                // subtitle's tail, which runs under the panel,
                // is dimmed to near-invisibility at the right.
                let strip_w = TOPIC_PANEL_W / 10.0;
                for i in 0..10 {
                    let alpha = 0.075 * (i + 1) as f32;
                    stage.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(scale, TOPIC_PANEL_X + i as f32 * strip_w),
                            top: px(scale, TOPIC_PANEL_Y),
                            width: px(scale, strip_w + 1.0),
                            height: px(scale, TOPIC_PANEL_H),
                            ..Default::default()
                        },
                        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, alpha)),
                    ));
                }
                // The topic list, scrollable and centered on
                // the selection. The selected row is 45 px
                // tall, the others 40.
                stage
                    .spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(scale, TOPIC_LIST_X),
                            top: px(scale, TOPIC_LIST_Y),
                            width: px(scale, TOPIC_LIST_W),
                            height: px(scale, TOPIC_VIEWPORT_H),
                            flex_direction: FlexDirection::Column,
                            overflow: Overflow::scroll_y(),
                            ..Default::default()
                        },
                        ScrollPosition(Vec2::new(0.0, scroll_y * scale)),
                    ))
                    .with_children(|list| {
                        for (i, topic) in topics.iter().enumerate() {
                            let is_selected = selected == Some(i);
                            let row_h = if is_selected {
                                TOPIC_ROW_SELECTED_H
                            } else {
                                TOPIC_ROW_H
                            };
                            list.spawn((
                                Node {
                                    width: Val::Percent(100.0),
                                    height: px(scale, row_h),
                                    flex_shrink: 0.0,
                                    align_items: AlignItems::Center,
                                    ..Default::default()
                                },
                                DialogueTopicRow { index: i },
                            ))
                            .observe(on_topic_click)
                            .with_children(|row| {
                                row.spawn((
                                    Text::new(topic.text.clone()),
                                    TextFont {
                                        font_size: fs(scale, TOPIC_FONT_SIZE),
                                        ..Default::default()
                                    },
                                    TextColor(if is_selected {
                                        Color::srgb(0.85, 0.7, 0.35)
                                    } else {
                                        Color::srgba(0.9, 0.9, 0.9, 1.0)
                                    }),
                                    TextLayout::no_wrap(),
                                ));
                            });
                        }
                    });
                // The Goodbye button, just below the last
                // visible topic row. No key prompt: the .swf's
                // button has a key-icon slot ([Tab]), but there
                // is no Tab key on a phone — the word is the tap
                // target (v0.4.13: "[Tab] Goodbye" on touch).
                stage
                    .spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(scale, EXIT_BUTTON_X),
                            top: px(scale, button_y),
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: px(scale, 6.0),
                            ..Default::default()
                        },
                        DialogueGoodbye,
                    ))
                    .observe(on_goodbye_click)
                    .with_children(|button| {
                        button.spawn((
                            Text::new("Goodbye"),
                            TextFont {
                                font_size: fs(scale, 20.0),
                                ..Default::default()
                            },
                            TextColor(Color::srgb(0.9, 0.85, 0.7)),
                        ));
                    });
            });
        });
}

/// A topic row was clicked: select it (the response shows via
/// the refresh system). The clicked entity is
/// `event.event_target()` — NOT `event.target()` (M17 lesson:
/// the old `target()` is the entity the observer is attached
/// to in a different sense, and the row is what we want).
fn on_topic_click(
    event: On<PointerClick>,
    rows: Query<&DialogueTopicRow>,
    mut state: ResMut<DialogueState>,
) {
    let Ok(row) = rows.get(event.event_target()) else {
        return;
    };
    if state.selected != Some(row.index) {
        state.selected = Some(row.index);
    }
}

/// The Goodbye button was clicked: close the conversation.
fn on_goodbye_click(
    _event: On<PointerClick>,
    mut commands: Commands,
    mut open: ResMut<DialogueOpen>,
    mut state: ResMut<DialogueState>,
    ui_roots: Query<Entity, With<DialogueUiRoot>>,
) {
    close_dialogue(&mut commands, &ui_roots, &mut open, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use dovah_esm::{Field, Record};

    /// Build a synthetic strings file: header + directory +
    /// data, with the given (id, text) entries. (Same builder
    /// as dovah-strings' own tests — the format is small enough
    /// to rebuild here.)
    fn build_file(entries: &[(u32, &str)], length_prefix: bool) -> Vec<u8> {
        let mut data = Vec::new();
        let mut dir = Vec::new();
        for (id, text) in entries {
            dir.push((*id, data.len() as u32));
            if length_prefix {
                let len = text.len() + 1;
                data.extend_from_slice(&(len as u32).to_le_bytes());
            }
            data.extend_from_slice(text.as_bytes());
            data.push(0);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        for (id, offset) in dir {
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
        }
        out.extend_from_slice(&data);
        out
    }

    fn test_strings() -> GameStrings {
        let general = StringsFile::from_bytes(
            &build_file(
                &[
                    (378, "Delphine"),
                    (1, "So, we need to find out what the Thalmor know."),
                ],
                false,
            ),
            StringsKind::General,
        )
        .unwrap();
        let dialogue = StringsFile::from_bytes(
            &build_file(
                &[(62071, "If we could get into the Thalmor Embassy...")],
                true,
            ),
            StringsKind::LengthPrefixed,
        )
        .unwrap();
        let longform = StringsFile::from_bytes(
            &build_file(&[(75140, "Daedra Heart")], true),
            StringsKind::LengthPrefixed,
        )
        .unwrap();
        GameStrings {
            sets: vec![StringsSet {
                general,
                dialogue,
                longform,
            }],
        }
    }

    fn field(code: &[u8; 4], data: &[u8]) -> Field {
        Field {
            field_type: *code,
            data: data.to_vec(),
        }
    }

    #[test]
    fn name_of_resolves_full_and_falls_back() {
        let strings = test_strings();
        let named = Record {
            record_type: *b"NPC_",
            flags: 0,
            form_id: 0x0001_3478,
            version: 44,
            fields: vec![field(b"FULL", &378u32.to_le_bytes())],
        };
        assert_eq!(strings.name_of(&named).as_deref(), Some("Delphine"));
        // An unresolvable ID falls back (the caller prettifies
        // the EDID) — name_of returns None, never a wrong name.
        let unknown = Record {
            record_type: *b"NPC_",
            flags: 0,
            form_id: 0x0001_3479,
            version: 44,
            fields: vec![field(b"FULL", &999_999u32.to_le_bytes())],
        };
        assert_eq!(strings.name_of(&unknown), None);
        // No FULL field at all.
        let bare = Record {
            record_type: *b"NPC_",
            flags: 0,
            form_id: 0x0001_3479,
            version: 44,
            fields: vec![],
        };
        assert_eq!(strings.name_of(&bare), None);
    }

    #[test]
    fn description_of_resolves_cnam_in_dlstrings() {
        // The real AtrFrgDaedricRecipe16 CNAM (75140 -> "Daedra
        // Heart", book_desc_probe) — the detail-pane text.
        let strings = test_strings();
        let book = Record {
            record_type: *b"BOOK",
            flags: 0,
            form_id: 0x000e_2a8d,
            version: 44,
            fields: vec![field(b"CNAM", &75140u32.to_le_bytes())],
        };
        assert_eq!(
            strings.description_of(&book).as_deref(),
            Some("Daedra Heart")
        );
        // CNAM 0 (most books) means no description.
        let plain = Record {
            record_type: *b"BOOK",
            flags: 0,
            form_id: 0x000f_456d,
            version: 44,
            fields: vec![field(b"CNAM", &0u32.to_le_bytes())],
        };
        assert_eq!(strings.description_of(&plain), None);
        // No CNAM field at all.
        let bare = Record {
            record_type: *b"BOOK",
            flags: 0,
            form_id: 0x000f_456d,
            version: 44,
            fields: vec![],
        };
        assert_eq!(strings.description_of(&bare), None);
    }

    #[test]
    fn set_for_falls_back_to_skyrim() {
        let strings = test_strings();
        // A Dragonborn form ID (top byte 4) with only one set
        // loaded falls back to the Skyrim set.
        assert_eq!(strings.set_for(0x0400_1234).general(378), Some("Delphine"));
    }

    #[test]
    fn facing_cone_is_45_degrees() {
        let feet = Vec3::new(0.0, 0.0, 0.0);
        // Forward is -Z (Bevy's forward).
        let forward = Vec3::new(0.0, 0.0, -1.0);
        // Dead ahead: faced.
        assert!(is_facing_target(feet, forward, Vec3::new(0.0, 0.0, -2.0)));
        // 30° off: faced (inside the 45° cone).
        assert!(is_facing_target(
            feet,
            forward,
            Vec3::new(1.0, 0.0, -1.732)
        ));
        // 60° off: not faced.
        assert!(!is_facing_target(
            feet,
            forward,
            Vec3::new(1.732, 0.0, -1.0)
        ));
        // Behind: not faced (the v0.4.13 Embry case — the NPC
        // is in range but the player looks at a chair).
        assert!(!is_facing_target(feet, forward, Vec3::new(0.0, 0.0, 2.0)));
        // At the player's feet: faced (no direction to check).
        assert!(is_facing_target(feet, forward, Vec3::new(0.0, 0.0, 0.0)));
        // Height does not matter (the range rule owns it).
        assert!(is_facing_target(feet, forward, Vec3::new(0.0, 5.0, -2.0)));
    }

    #[test]
    fn facing_search_skips_unfaced_npcs() {
        let mut talkable = TalkableNpcs::default();
        // Embry, 2 m ahead; Orgnar, 1 m behind (nearer, but
        // unfaced — the proximity-only search would pick him).
        talkable.npcs.insert(
            0x3550b,
            TalkableNpc {
                base_form_id: 0x3550b,
                position: Vec3::new(0.0, 0.0, -2.0),
                base_edid: "Embry".to_string(),
            },
        );
        talkable.npcs.insert(
            0x13479,
            TalkableNpc {
                base_form_id: 0x13479,
                position: Vec3::new(0.0, 0.0, 1.0),
                base_edid: "Orgnar".to_string(),
            },
        );
        let feet = Vec3::ZERO;
        let forward = Vec3::new(0.0, 0.0, -1.0);
        // Proximity-only still picks the nearer (behind) NPC —
        // the spent-checks in interact.rs rely on that.
        assert_eq!(nearest_npc_in_range(&talkable, feet), Some(0x13479));
        // Facing-aware picks the faced one, farther away.
        assert_eq!(
            nearest_npc_in_range_facing(&talkable, feet, forward),
            Some(0x3550b)
        );
        // Facing away from both: none.
        assert_eq!(
            nearest_npc_in_range_facing(&talkable, feet, Vec3::new(1.0, 0.0, 0.0)),
            None
        );
    }

    #[test]
    fn current_line_shows_greeting_then_response() {
        let mut state = DialogueState {
            npc_ref: Some(1),
            npc_name: "Delphine".to_string(),
            greeting: "What do you need, Dragonborn?".to_string(),
            topics: vec![DialogueTopic {
                dial_form_id: 2,
                text: "So, we need to find out what the Thalmor know.".to_string(),
                response: "If we could get into the Thalmor Embassy...".to_string(),
            }],
            selected: None,
        };
        assert_eq!(state.current_line(), "What do you need, Dragonborn?");
        state.selected = Some(0);
        assert_eq!(
            state.current_line(),
            "If we could get into the Thalmor Embassy..."
        );
        // An out-of-range selection degrades to the greeting.
        state.selected = Some(7);
        assert_eq!(state.current_line(), "What do you need, Dragonborn?");
    }

    #[test]
    fn nearest_npc_in_range_picks_closest() {
        let mut talkable = TalkableNpcs::default();
        talkable.npcs.insert(
            10,
            TalkableNpc {
                base_form_id: 0x0001_3478,
                position: Vec3::new(2.0, 0.0, 0.0),
                base_edid: "Delphine".to_string(),
            },
        );
        talkable.npcs.insert(
            11,
            TalkableNpc {
                base_form_id: 0x0001_3479,
                position: Vec3::new(1.0, 0.0, 0.0),
                base_edid: "Orgnar".to_string(),
            },
        );
        talkable.npcs.insert(
            12,
            TalkableNpc {
                base_form_id: 0x0003_550b,
                position: Vec3::new(9.0, 0.0, 0.0),
                base_edid: "Embry".to_string(),
            },
        );
        // Orgnar is closest; Embry is out of range.
        assert_eq!(nearest_npc_in_range(&talkable, Vec3::ZERO), Some(11));
        // Nobody in range from far away.
        assert_eq!(
            nearest_npc_in_range(&talkable, Vec3::new(50.0, 0.0, 0.0)),
            None
        );
    }

    #[test]
    fn dialogue_layout_constants_match_the_swf() {
        // dialoguemenu.swf: menu origin (599.5, 309.7), stage
        // 1280x720; the constants are the absolute stage
        // coordinates derived from the dump. Float sums are
        // compared with a 0.01 tolerance — exact `assert_eq!`
        // on f32 sums is fragile (599.5 + 148.1 != 747.6 in
        // binary).
        let close = |a: f32, b: f32| assert!((a - b).abs() < 0.01, "{a} != {b}");
        close(TOPIC_PANEL_X, 599.5 + 148.1);
        close(TOPIC_PANEL_Y, 309.7 - 220.0);
        close(TOPIC_PANEL_W, 1280.0 - TOPIC_PANEL_X);
        close(TOPIC_PANEL_H, 720.0 - TOPIC_PANEL_Y);
        close(TOPIC_LIST_X, TOPIC_PANEL_X + 50.0);
        close(TOPIC_LIST_Y, TOPIC_PANEL_Y + 270.0);
        assert_eq!(TOPIC_SELECTED_Y, 160.0);
        assert_eq!(TOPIC_VIEWPORT_H, 285.0 + TOPIC_ROW_H);
        close(SUBTITLE_X, 599.5 - 351.0);
        close(SUBTITLE_Y, 309.7 + 267.0);
        close(SPEAKER_X, 599.5 - 53.0);
        close(SPEAKER_Y, 309.7 + 126.2);
        close(EXIT_BUTTON_X, 599.5 + 345.0);
        // The speaker name is centered on the subtitle.
        let subtitle_center = SUBTITLE_X + SUBTITLE_W / 2.0;
        let speaker_center = SPEAKER_X + SPEAKER_W / 2.0;
        assert!((subtitle_center - speaker_center).abs() < 3.0);
    }

    #[test]
    fn exit_button_sits_below_the_last_row() {
        // Four topics: the button rises to just below row 4.
        assert_eq!(
            exit_button_y(4),
            TOPIC_LIST_Y + 4.0 * TOPIC_ROW_H + EXIT_BUTTON_GAP
        );
        // A full list (8 rows): capped, clearing the 8th row.
        assert_eq!(exit_button_y(8), EXIT_BUTTON_MAX_Y);
        assert!(exit_button_y(8) >= TOPIC_LIST_Y + 8.0 * TOPIC_ROW_H);
        // The .swf's fixed 669.7 overlaps the 8th row by 7 px
        // — the cap sits below the row instead.
        assert!(exit_button_y(8) > 669.7);
    }

    #[test]
    fn dialogue_plugin_builds_and_runs_two_updates() {
        // The M17 lesson, applied to dialogue: the plugin's
        // systems must not panic in a minimal app with no
        // scene, no plugin, and no strings loaded.
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .add_plugins(bevy::input::InputPlugin)
            .add_plugins(bevy::asset::AssetPlugin::default());
        app.add_plugins(DialoguePlugin);
        app.update();
        app.update();
        assert!(!app.world().resource::<DialogueOpen>().0);
    }

    /// A placed actor for the talkable tests: Skyrim-space
    /// position, creature flag as the loader would set it.
    fn placed_actor(
        ref_form_id: u32,
        base_form_id: u32,
        base_edid: &str,
        is_creature: bool,
        position: [f32; 3],
    ) -> crate::cell::PlacedObject {
        crate::cell::PlacedObject {
            ref_form_id,
            base_form_id,
            base_type: "NPC_".to_string(),
            base_edid: base_edid.to_string(),
            model_path: None,
            position,
            rotation: [0.0, 0.0, 0.0],
            scale: 1.0,
            bounds: None,
            light: None,
            is_actor: true,
            is_creature,
            actor_models: Vec::new(),
            xtel: None,
        }
    }

    fn cell_with(objects: Vec<crate::cell::PlacedObject>) -> crate::cell::CellData {
        crate::cell::CellData {
            edid: "TestCell".to_string(),
            form_id: 0x1234,
            interior: false,
            objects,
            lands: Vec::new(),
            waters: Vec::new(),
            spawn: None,
            doors: Vec::new(),
            skipped_markers: 0,
            skipped_disabled: 0,
            skipped_no_data: 0,
            skipped_no_base: 0,
            skipped_no_model: 0,
            skipped_other_types: 0,
            skipped_effects: 0,
        }
    }

    /// The chicken dialogue bug, pinned: the Riverwood chicken
    /// (ref 0x001066de, base EncChicken 0x000a91a0) is a
    /// creature with no dialogue — it must NOT be talkable,
    /// even when it is nearer to the player's feet than the man
    /// (the guard at 2 m, the chicken at 1 m). A creature WITH
    /// dialogue (the dragon, with a greeting) stays talkable —
    /// Paarthurnax talks. The picker then hands USE to the man,
    /// not the chicken.
    #[test]
    fn build_talkable_npcs_excludes_creatures_but_keeps_talking_ones() {
        use bevy::app::{App, Update};

        let cell = cell_with(vec![
            // The chicken, 1 m from the origin — nearer than the man.
            placed_actor(
                0x0010_66de,
                0x000a_91a0,
                "EncChicken",
                true,
                [70.0, 0.0, 0.0],
            ),
            // The man, 2 m from the origin.
            placed_actor(
                0x0001_b743,
                0x0002_74ab,
                "GuardRiverwoodImperial01",
                false,
                [140.0, 0.0, 0.0],
            ),
            // The dragon, 10 m away, a creature with a greeting.
            placed_actor(
                0x0003_c580,
                0x0003_c57f,
                "Paarthurnax",
                true,
                [700.0, 0.0, 0.0],
            ),
        ]);
        let mut index = DialogueIndex::default();
        index
            .greetings
            .insert(0x0003_c57f, "Dovahkiin, greeted am I.".to_string());
        index.built = true;

        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .init_resource::<TalkableNpcs>()
            .insert_resource(cell)
            .insert_resource(index)
            .add_systems(Update, build_talkable_npcs);
        app.update();

        let talkable = app.world().resource::<TalkableNpcs>();
        assert!(
            !talkable.npcs.contains_key(&0x0010_66de),
            "the chicken must not be talkable"
        );
        assert!(talkable.npcs.contains_key(&0x0001_b743));
        assert!(
            talkable.npcs.contains_key(&0x0003_c580),
            "a creature with dialogue stays talkable"
        );
        // The picker: the chicken is nearer, but it is not in
        // the map — USE goes to the man.
        assert_eq!(
            nearest_npc_in_range(talkable, Vec3::ZERO),
            Some(0x0001_b743)
        );
    }

    /// Without the dialogue index (or before it builds), the
    /// race rule alone applies: the dragon is out too — there
    /// is no evidence it talks yet. The chain builds the index
    /// before the talkable list, so this is the fallback, not
    /// the normal path.
    #[test]
    fn build_talkable_npcs_without_index_uses_the_race_rule() {
        use bevy::app::{App, Update};

        let cell = cell_with(vec![
            placed_actor(
                0x0010_66de,
                0x000a_91a0,
                "EncChicken",
                true,
                [70.0, 0.0, 0.0],
            ),
            placed_actor(
                0x0001_b743,
                0x0002_74ab,
                "GuardRiverwoodImperial01",
                false,
                [140.0, 0.0, 0.0],
            ),
            placed_actor(
                0x0003_c580,
                0x0003_c57f,
                "Paarthurnax",
                true,
                [700.0, 0.0, 0.0],
            ),
        ]);

        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .init_resource::<TalkableNpcs>()
            .insert_resource(cell)
            .add_systems(Update, build_talkable_npcs);
        app.update();

        let talkable = app.world().resource::<TalkableNpcs>();
        assert!(!talkable.npcs.contains_key(&0x0010_66de));
        assert!(!talkable.npcs.contains_key(&0x0003_c580));
        assert!(talkable.npcs.contains_key(&0x0001_b743));
    }
}
