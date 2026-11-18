//! The original pause screen (UI milestone 1): Skyrim's Esc
//! menu — Quests, Stats, System — built from the game's own
//! `quest_journal.swf` geometry (see
//! `interface-extract/study/pause_hud_study.md`), plus the
//! settings it wires for real and a minimal save file.
//!
//! Dylan's correction, proven by the .swf: the pause screen has
//! THREE tabs — Quests, Stats, System. Map/Magic/Items/Skills
//! are a different menu (Items is already done vanilla; the
//! others follow later). The pause screen is quests, settings,
//! save, load, quit.
//!
//! ## What is real here, and what is honestly not
//!
//! * Quests: the REAL quest list from the ESM (QUST records,
//!   names via `.STRINGS`, objectives via `.STRINGS`, stage log
//!   text via `.DLSTRINGS` — see `dovah-esm::quest`). The list
//!   shows named quests that have journal content (objectives
//!   or stage log entries); the 525 unnamed system quests and
//!   the named quests with no journal content are not journal
//!   quests. NO quest is marked active or completed — quest
//!   state needs the quest system, which does not exist yet.
//!   The detail pane shows the quest's FIRST stage log entry
//!   (its opening text) and ALL its objectives (stage gating
//!   needs the quest system too).
//! * Stats: only values the engine actually knows (level,
//!   health/magicka/stamina, gold, carry weight, days passed,
//!   quests completed, locations discovered — the last three
//!   are honestly 0: no time system, no quest system, no
//!   discovery system yet). One category (General Stats) —
//!   the others need their tracking systems.
//! * Settings: Look Sensitivity (stepper, wired to mouse look)
//!   and Invert Y (checkbox, wired to mouse look) are REAL and
//!   persist to `settings.json` in the save dir. Display and
//!   Audio say plainly they have no settings yet — no display
//!   or audio system exists to configure. Nothing is faked.
//! * Save: writes a REAL save file (JSON: cell, position,
//!   inventory, stamina, settings — see `crate::save` for the
//!   full format). Load lists the saves and loads the selected
//!   one for real: the save's cell loads with the saved
//!   position as the spawn point, and the inventory, stats,
//!   spells, skills, doors, and world changes are restored.
//! * Quit: quits to desktop for real (`AppExit`).
//! * The bottom bar shows the game's start date (17th of Last
//!   Seed, 4E 201) — static, because no time system advances
//!   the clock yet — and LEVEL 1 with an empty progress bar
//!   (no XP system yet).
//!
//! ## Layout
//!
//! Stage coords from `quest_journal.swf` (1280x720), scaled by
//! the shared `UiScale` — the same `px()`/`fs()` helpers as the
//! inventory. The panel background gradient is unrecoverable
//! from the .swf (same as the dialogue ListPanel), so the
//! panels use the inventory's flat 60% black + grey border.

use bevy::app::{App, Plugin, Startup, Update};
use bevy::ecs::prelude::*;
use bevy::input::keyboard::KeyCode;
use bevy::input::ButtonInput;
use bevy::picking::events::PointerClick;
use bevy::text::{TextColor, TextFont, TextLayout};
use bevy::ui::widget::Text;
use bevy::ui::{
    AlignItems, BackgroundColor, BorderColor, FlexDirection, JustifyContent, Node, Overflow,
    PositionType, ScrollPosition, UiRect, Val,
};
use serde::{Deserialize, Serialize};

use crate::dialogue::GameStrings;
use crate::inventory::{Inventory, UiScale, STAGE_HEIGHT};
use crate::player::Stamina;
use crate::save::{list_saves, SaveDir, SaveList, SaveRequested};
use dovah_esm::parse_quest;

// ---------------------------------------------------------------------------
// Layout constants (stage px, from quest_journal.swf — see the study)
// ---------------------------------------------------------------------------

/// Tabs: Quests (300), Stats (566), System (830), all at y 53.5;
/// tab text 154x32, size 24.
const TAB_Y: f32 = 53.5;
const TAB_W: f32 = 154.0;
const TAB_FONT: f32 = 24.0;
/// Page origin Y: Quests/Stats/System faders all at y 104.3
/// (the X origins — 193.1 for Quests/Stats, 167.1 for System —
/// live in the list constants below).
const PAGE_Y: f32 = 104.3;
/// The big panel backdrop (shape 594: 966x552 at (157.1, 46.3)).
const PANEL_X: f32 = 157.1;
const PANEL_Y: f32 = 46.3;
const PANEL_W: f32 = 966.0;
const PANEL_H: f32 = 552.0;

/// Bottom bar band: y 650-720 (JournalBottomBar at (0,720),
/// band shape y[-70,56]).
const BOTTOM_BAR_Y: f32 = 650.0;
/// Date text at (895, 669.2), 300x28, size 20.
const DATE_X: f32 = 895.0;
const DATE_Y: f32 = 669.2;
const DATE_FONT: f32 = 20.0;
/// Bottom-bar buttons at (90, 668); icon at -28, text 150x28,
/// size 20.
const BAR_BUTTONS_X: f32 = 90.0;
const BAR_BUTTONS_Y: f32 = 668.0;
const BAR_BUTTON_FONT: f32 = 20.0;
/// Level meter: "LEVEL" label (449.4, 672) size 16, number
/// (504.8, 666.5) size 24, bar 220x22 at (642.2, 681.4).
const LEVEL_LABEL_X: f32 = 449.4;
const LEVEL_LABEL_Y: f32 = 672.0;
const LEVEL_NUM_X: f32 = 504.8;
const LEVEL_NUM_Y: f32 = 666.5;
const LEVEL_BAR_X: f32 = 642.2;
const LEVEL_BAR_Y: f32 = 681.4;
const LEVEL_BAR_W: f32 = 220.0;
const LEVEL_BAR_H: f32 = 22.0;

/// Quest list at (193.1, 126.3); entries 25px pitch, text
/// 230x23, size 16; 17 entry slots (425px viewport).
const QUEST_LIST_X: f32 = 193.1;
const QUEST_LIST_Y: f32 = 126.3;
const QUEST_ROW_H: f32 = 25.0;
const QUEST_ROW_W: f32 = 230.0;
const QUEST_ROW_FONT: f32 = 16.0;
const QUEST_VISIBLE_ROWS: usize = 17;
/// Quest divider at x 447.1, 15x487.
const QUEST_DIVIDER_X: f32 = 447.1;
/// Quest title at y 129.3, size 24. The .swf field is 220x32
/// at x 682.7, but the title spans the description width (see
/// build_quests_content) — QUEST_TITLE_X is recorded here only.
const QUEST_TITLE_Y: f32 = 129.3;
const QUEST_TITLE_FONT: f32 = 20.0;
/// Quest description at (493.9, 173.3), 603x32, size 24 — one
/// line, clipped like the game.
const QUEST_DESC_X: f32 = 493.9;
const QUEST_DESC_Y: f32 = 173.3;
const QUEST_DESC_W: f32 = 603.0;
const QUEST_DESC_FONT: f32 = 24.0;
/// Objectives header at (551.1, 244.3); list at (492.8, 286.3),
/// entries 35px pitch, text 590x32 at x+22, size 16.
const OBJECTIVES_HEADER_X: f32 = 551.1;
const OBJECTIVES_HEADER_Y: f32 = 244.3;
const OBJECTIVES_X: f32 = 492.8;
const OBJECTIVES_Y: f32 = 286.3;
const OBJECTIVE_ROW_H: f32 = 35.0;
/// Objective text size. The .swf says 24, but at 24 Bevy's
/// wider font clips most objective sentences mid-phrase in the
/// 545px field ("Speak to Orthas Endario about the" — proven by
/// the first proof render); 20 fit the common ones whole on
/// desktop, but the phone proved otherwise — "Speak to Orthas
/// Endario about the East Empire Company's..." wrapped at 20px
/// and showed a half-cut second line (v0.4.13). 16 in the
/// widened 590px field fits it on one line. Same judgment as
/// the dialogue topics' 24 -> 18.
const OBJECTIVE_FONT: f32 = 16.0;
/// Objective marker: 11x17 diamond at x 4-16 — drawn as a
/// small square (no diamond glyph in the default font).
const OBJECTIVE_MARKER_SIZE: f32 = 8.0;

/// System category list at (167.1, 151.3); entries 25px pitch,
/// text 230x23, size 16. Divider at x 402.1, 15x487.
const SYSTEM_CAT_X: f32 = 167.1;
const SYSTEM_CAT_Y: f32 = 151.3;
const SYSTEM_DIVIDER_X: f32 = 402.1;
/// Version text at (184.1, 560.9), size 12.
const VERSION_X: f32 = 184.1;
const VERSION_Y: f32 = 560.9;
/// System panels at x 442.1: Settings y 75, Options y 70,
/// Quit y 210 (panel-local, + SystemPage origin 104.3).
const SYSTEM_PANEL_X: f32 = 442.1;
const SETTINGS_PANEL_Y: f32 = 179.3;
const OPTIONS_PANEL_Y: f32 = 174.3;
const QUIT_PANEL_Y: f32 = 314.3;
/// Settings categories: 3 entries at 35px pitch, text 200x30,
/// size 22. Option rows: 40px pitch, label 125x30 size 22,
/// stepper at x 135.8 (value 130x29 size 20), checkbox at x
/// 215.8.
const SETTINGS_CAT_ROW_H: f32 = 35.0;
const SETTINGS_CAT_FONT: f32 = 22.0;
const OPTION_ROW_H: f32 = 40.0;
const OPTION_LABEL_FONT: f32 = 20.0;
const OPTION_VALUE_FONT: f32 = 20.0;
/// Stepper/checkbox x, relative to the panel. The .swf puts the
/// stepper at 135.8 and the checkbox at 215.8, with a 125px label
/// field — but in Bevy's wider font "Look Sensitivity" needs
/// ~190px, so the .swf label field wraps it onto the next row
/// (proven by the first proof render). The label field is 200px
/// and both controls move to 210, so every label reads whole.
const OPTION_STEPPER_X: f32 = 210.0;
const OPTION_CHECKBOX_X: f32 = 210.0;
/// Save/load list: entries 27px pitch (the .swf's x 25 is the
/// list's own left edge inside the panel), name 265x25 at x 52,
/// size 18.
const SAVE_ROW_H: f32 = 27.0;
const SAVE_FONT: f32 = 18.0;
const SAVE_VISIBLE_ROWS: usize = 11;
/// Quit list: 2 entries at 35px pitch, text 185x32, size 24.
const QUIT_ROW_H: f32 = 35.0;
const QUIT_FONT: f32 = 24.0;
/// Stats: category list at (193.1, 199.3); stats list at
/// (458.1, 154.3), entries 35px pitch at x 25, label 350x30 +
/// value 220x30 at x 352, size 22. Divider x[428.1, 443.1].
const STATS_CAT_X: f32 = 193.1;
const STATS_CAT_Y: f32 = 199.3;
const STATS_LIST_X: f32 = 458.1;
const STATS_LIST_Y: f32 = 154.3;
const STAT_ROW_H: f32 = 35.0;
const STAT_FONT: f32 = 22.0;
const STATS_DIVIDER_X: f32 = 428.1;

/// Colors (same .swf values as the inventory): gold highlight,
/// 60% black panel, grey divider/border, dim white text.
const GOLD: bevy::color::Color = bevy::color::Color::srgb(0.85, 0.7, 0.35);
const PANEL_BG: bevy::color::Color = bevy::color::Color::srgba(0.0, 0.0, 0.0, 0.6);
const PANEL_BORDER: bevy::color::Color = bevy::color::Color::srgba(0.6, 0.6, 0.6, 0.6);
const DIVIDER_COLOR: bevy::color::Color = bevy::color::Color::srgba(0.6, 0.6, 0.6, 0.5);
const TEXT_DIM: bevy::color::Color = bevy::color::Color::srgba(1.0, 1.0, 1.0, 0.7);
const TEXT_WHITE: bevy::color::Color = bevy::color::Color::WHITE;
/// The world-dimming fader behind the menu.
const FADER_BG: bevy::color::Color = bevy::color::Color::srgba(0.0, 0.0, 0.0, 0.55);

/// The game's start date, shown in the bottom bar. Static — no
/// time system advances the clock yet.
pub(crate) const GAME_START_DATE: &str = "17th of Last Seed, 4E 201";

/// Stage px -> scaled px / font size (the inventory's shared
/// helpers, same scale).
use crate::inventory::{fs, px};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Whether the pause screen is open (the same gate pattern as
/// `InventoryOpen` / `DialogueOpen`: movement and mouse look
/// freeze while it is).
#[derive(Debug, Default, Resource)]
pub struct PauseOpen(pub bool);

/// A cross-menu request to open the pause screen (UI milestone
/// 2): the map tab's Journal button. Consumed (reset to `false`)
/// by `apply_open_pause_request`.
#[derive(Debug, Default, Resource)]
pub struct OpenPauseRequest(pub bool);

/// Which pause tab is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PauseTab {
    #[default]
    Quests,
    Stats,
    System,
}

impl PauseTab {
    /// Tab x position (stage px) and label.
    fn geometry(self) -> (f32, &'static str) {
        match self {
            PauseTab::Quests => (300.0, "QUESTS"),
            PauseTab::Stats => (566.0, "STATS"),
            PauseTab::System => (830.0, "SYSTEM"),
        }
    }

    /// Parse a `--pause-tab` name.
    pub fn from_name(name: &str) -> Option<PauseTab> {
        match name.to_ascii_lowercase().as_str() {
            "quests" => Some(PauseTab::Quests),
            "stats" => Some(PauseTab::Stats),
            "system" | "settings" | "controls" | "load" | "save" | "help" | "quit" => {
                Some(PauseTab::System)
            }
            _ => None,
        }
    }
}

/// Which System category is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PauseSystem {
    #[default]
    Settings,
    Controls,
    Load,
    Save,
    Help,
    Quit,
}

impl PauseSystem {
    pub const ALL: [PauseSystem; 6] = [
        PauseSystem::Settings,
        PauseSystem::Controls,
        PauseSystem::Load,
        PauseSystem::Save,
        PauseSystem::Help,
        PauseSystem::Quit,
    ];

    fn label(self) -> &'static str {
        match self {
            PauseSystem::Settings => "Settings",
            PauseSystem::Controls => "Controls",
            PauseSystem::Load => "Load",
            PauseSystem::Save => "Save",
            PauseSystem::Help => "Help",
            PauseSystem::Quit => "Quit",
        }
    }

    /// Parse a `--pause-tab` system name (settings/controls/
    /// load/save/help/quit imply the System tab).
    pub fn from_name(name: &str) -> Option<PauseSystem> {
        match name.to_ascii_lowercase().as_str() {
            "settings" => Some(PauseSystem::Settings),
            "controls" => Some(PauseSystem::Controls),
            "load" => Some(PauseSystem::Load),
            "save" => Some(PauseSystem::Save),
            "help" => Some(PauseSystem::Help),
            "quit" => Some(PauseSystem::Quit),
            _ => None,
        }
    }
}

/// Which settings category is showing its options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsCategory {
    #[default]
    Gameplay,
    Display,
    Audio,
}

impl SettingsCategory {
    pub const ALL: [SettingsCategory; 3] = [
        SettingsCategory::Gameplay,
        SettingsCategory::Display,
        SettingsCategory::Audio,
    ];

    fn label(self) -> &'static str {
        match self {
            SettingsCategory::Gameplay => "Gameplay",
            SettingsCategory::Display => "Display",
            SettingsCategory::Audio => "Audio",
        }
    }
}

/// The settings panel has two levels: the category list, then
/// the selected category's options (the .swf's SettingsPanel /
/// OptionsListsPanel swap). Tab backs out of the options to
/// the categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsView {
    #[default]
    Categories,
    Options,
}

/// All pause navigation state in one resource (keeps the
/// rebuild system under Bevy's 16-parameter limit).
#[derive(Debug, Resource)]
pub struct PauseState {
    pub tab: PauseTab,
    /// Selected quest index (into `QuestIndex.entries`).
    pub quest: usize,
    /// Selected System category.
    pub system: PauseSystem,
    /// Selected settings category (when the Settings panel is
    /// showing its categories).
    pub settings_category: SettingsCategory,
    /// Categories list vs options list (Settings panel only).
    pub settings_view: SettingsView,
    /// Selected option row (Gameplay options only — Display /
    /// Audio have no options yet).
    pub setting: usize,
    /// Selected save entry (Load / Save panels).
    pub save: usize,
    /// A load was requested from the Load panel (E key) —
    /// consumed by the save system's `apply_pause_load_request`
    /// (this struct is at Bevy's 16-parameter limit in
    /// `pause_keys`, so the request travels through state).
    pub load_requested: bool,
    /// Selected quit entry (0 = Quit to Desktop, 1 = Cancel).
    pub quit: usize,
    /// Selected help topic.
    pub help: usize,
}

impl Default for PauseState {
    fn default() -> Self {
        PauseState {
            tab: PauseTab::Quests,
            quest: 0,
            system: PauseSystem::Settings,
            settings_category: SettingsCategory::Gameplay,
            settings_view: SettingsView::Categories,
            setting: 0,
            save: 0,
            load_requested: false,
            quit: 1, // Cancel is the safe default.
            help: 0,
        }
    }
}

/// The UI needs a rebuild (a state change since the last
/// build). Set by the key/touch handlers; consumed by
/// `refresh_pause_ui`.
#[derive(Debug, Default, Resource)]
pub struct PauseDirty(pub bool);

/// Screenshot-proof seed (`--pause` / `--pause-tab`): which
/// tab (and System category) the shot opens on. Inserted only
/// on the screenshot path; absent everywhere else.
#[derive(Debug, Clone, Resource)]
pub struct PauseSeed {
    pub tab: PauseTab,
    pub system: PauseSystem,
    /// Open the Settings panel on this category's options
    /// (only meaningful with tab System + system Settings).
    pub settings_options: Option<SettingsCategory>,
}

impl PauseSeed {
    /// Build a seed from a `--pause-tab` name: quests / stats /
    /// system / settings / controls / load / save / help / quit,
    /// plus gameplay / display / audio (which open the Settings
    /// panel on that category's options directly — the proof
    /// shots for the wired settings and the honest empty ones).
    /// "settings" opens the Gameplay options (the wired
    /// settings are the proof); the categories list is one Tab
    /// press away.
    pub fn from_name(name: &str) -> Option<PauseSeed> {
        let lower = name.to_ascii_lowercase();
        let settings_options = match lower.as_str() {
            "settings" | "gameplay" => Some(SettingsCategory::Gameplay),
            "display" => Some(SettingsCategory::Display),
            "audio" => Some(SettingsCategory::Audio),
            _ => None,
        };
        // The settings-category names imply System + Settings.
        if matches!(lower.as_str(), "gameplay" | "display" | "audio") {
            return Some(PauseSeed {
                tab: PauseTab::System,
                system: PauseSystem::Settings,
                settings_options,
            });
        }
        let tab = PauseTab::from_name(name)?;
        let system = PauseSystem::from_name(name).unwrap_or(PauseSystem::Settings);
        Some(PauseSeed {
            tab,
            system,
            settings_options,
        })
    }
}

// ---------------------------------------------------------------------------
// Game settings (the real ones)
// ---------------------------------------------------------------------------

/// Look sensitivity steps (the .swf stepper): multipliers on
/// the base 0.0025 rad/px. Default 100%.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Sensitivity {
    Half,
    ThreeQuarters,
    #[default]
    Normal,
    OneHalf,
    Double,
}

impl Sensitivity {
    pub const ALL: [Sensitivity; 5] = [
        Sensitivity::Half,
        Sensitivity::ThreeQuarters,
        Sensitivity::Normal,
        Sensitivity::OneHalf,
        Sensitivity::Double,
    ];

    /// Multiplier on the base sensitivity.
    pub fn multiplier(self) -> f32 {
        match self {
            Sensitivity::Half => 0.5,
            Sensitivity::ThreeQuarters => 0.75,
            Sensitivity::Normal => 1.0,
            Sensitivity::OneHalf => 1.5,
            Sensitivity::Double => 2.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Sensitivity::Half => "50%",
            Sensitivity::ThreeQuarters => "75%",
            Sensitivity::Normal => "100%",
            Sensitivity::OneHalf => "150%",
            Sensitivity::Double => "200%",
        }
    }

    fn next(self) -> Sensitivity {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(2);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    fn prev(self) -> Sensitivity {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(2);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// The settings the engine actually supports. Read by
/// `player::mouse_look` (sensitivity + invert Y) and persisted
/// to `settings.json` in the save dir.
#[derive(Debug, Clone, PartialEq, Resource, Serialize, Deserialize)]
pub struct GameSettings {
    pub sensitivity: Sensitivity,
    pub invert_y: bool,
    /// The real Skyrim HUD (Ruffle) instead of the Bevy HUD (M4).
    /// Old settings files lack the field, so it defaults to true
    /// (the post-verification default). The screenshot path ignores
    /// this and is driven by the `--ruffle-hud` CLI flag instead.
    #[serde(default = "default_ruffle_hud")]
    pub ruffle_hud: bool,
}

/// The `ruffle_hud` default for settings files written before M4.
fn default_ruffle_hud() -> bool {
    true
}

impl Default for GameSettings {
    fn default() -> Self {
        GameSettings {
            sensitivity: Sensitivity::Normal,
            invert_y: false,
            ruffle_hud: true,
        }
    }
}

/// Settings file name inside the save dir.
const SETTINGS_FILE: &str = "settings.json";

/// Load persisted settings (best-effort: any failure keeps the
/// defaults — a corrupt settings file must never block the
/// menu).
pub fn load_settings(dir: &std::path::Path) -> GameSettings {
    std::fs::read_to_string(dir.join(SETTINGS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Persist settings (best-effort: logs and continues on
/// failure — settings are not worth crashing over).
pub fn save_settings(dir: &std::path::Path, settings: &GameSettings) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(dir.join(SETTINGS_FILE), text);
    }
}

// ---------------------------------------------------------------------------
// Quest index
// ---------------------------------------------------------------------------

/// One journal quest: real name, opening log text, objectives.
/// No state — active/completed needs the quest system.
#[derive(Debug, Clone)]
pub struct QuestEntry {
    /// The quest's form ID — unread until the quest system
    /// needs to open/track a quest by ID.
    #[allow(dead_code)]
    pub form_id: u32,
    pub name: String,
    /// The quest's FIRST stage log entry in file order (its
    /// opening text), or "" when it has none.
    pub description: String,
    /// Objective texts, in file order (unresolvable NNAMs
    /// skipped — 11 in Skyrim.esm).
    pub objectives: Vec<String>,
}

/// The journal's quest list, built once per app run from the
/// parsed plugin + strings. Only named quests with journal
/// content (objectives or stage log entries) are listed — the
/// 525 unnamed system quests and the named quests with no
/// journal content have nothing to show.
#[derive(Debug, Default, Resource)]
pub struct QuestIndex {
    pub entries: Vec<QuestEntry>,
    pub built: bool,
}

impl QuestIndex {
    pub fn build(plugin: &dovah_esm::Plugin, strings: &GameStrings) -> QuestIndex {
        let mut entries = Vec::new();
        for record in plugin.all_records() {
            if &record.record_type != b"QUST" {
                continue;
            }
            let Some(quest) = parse_quest(record) else {
                continue;
            };
            let Some(full_id) = quest.full_id else {
                continue;
            };
            let set = strings.set_for(record.form_id);
            let Some(name) = set.general(full_id).map(|s| s.to_string()) else {
                continue;
            };
            let objectives: Vec<String> = quest
                .objectives
                .iter()
                .filter_map(|o| set.general(o.text_id).map(|s| s.to_string()))
                .collect();
            let description = quest
                .stage_texts
                .iter()
                .find_map(|s| set.longform(s.text_id).map(|t| t.to_string()))
                .unwrap_or_default();
            if objectives.is_empty() && description.is_empty() {
                continue;
            }
            entries.push(QuestEntry {
                form_id: quest.form_id,
                name,
                description,
                objectives,
            });
        }
        QuestIndex {
            entries,
            built: true,
        }
    }
}

/// Build the quest index (once per app run). Runs in the
/// scene-load chain, after the dialogue index.
pub fn build_quest_index(
    plugin: Option<Res<crate::SharedPlugin>>,
    strings: Option<Res<GameStrings>>,
    mut index: ResMut<QuestIndex>,
) {
    let (Some(plugin), Some(strings)) = (plugin, strings) else {
        return;
    };
    if index.built {
        return;
    }
    let started = std::time::Instant::now();
    *index = QuestIndex::build(&plugin.0, &strings);
    crate::startup_log::log(&format!(
        "quest index built: {} journal quests ({} ms)",
        index.entries.len(),
        started.elapsed().as_millis()
    ));
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

/// Marker for the pause UI root (despawned on close / rebuild).
#[derive(Component)]
pub struct PauseUiRoot;

/// Marker for a tab button.
#[derive(Component)]
struct PauseTabButton {
    tab: PauseTab,
}

/// Marker for one quest row.
#[derive(Component)]
struct PauseQuestRow {
    index: usize,
}

/// Marker for one System category row.
#[derive(Component)]
struct PauseSystemRow {
    system: PauseSystem,
}

/// Marker for one settings-category row.
#[derive(Component)]
struct PauseSettingsCategoryRow {
    category: SettingsCategory,
}

/// Marker for one Gameplay option row (0 = sensitivity, 1 =
/// invert Y).
#[derive(Component)]
struct PauseOptionRow {
    index: usize,
}

/// Marker for one save row (Load panel).
#[derive(Component)]
struct PauseSaveRow {
    index: usize,
}

/// Marker for one quit row (0 = Quit to Desktop, 1 = Cancel).
#[derive(Component)]
struct PauseQuitRow {
    index: usize,
}

/// Marker for one help topic row.
#[derive(Component)]
struct PauseHelpRow {
    index: usize,
}

/// Marker for the bottom-bar Close button.
#[derive(Component)]
struct PauseCloseButton;

/// Marker for the Save panel's Save Game button.
#[derive(Component)]
struct PauseSaveButton;

/// Marker for the Load panel's Load Game button.
#[derive(Component)]
struct PauseLoadButton;

/// The rebuild's inputs, bundled so the system stays under
/// Bevy's 16-parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
struct PauseUiParams<'w> {
    scale: Res<'w, UiScale>,
    index: Res<'w, QuestIndex>,
    state: Res<'w, PauseState>,
    settings: Res<'w, GameSettings>,
    inventory: Res<'w, Inventory>,
    catalog: Res<'w, crate::inventory::ItemCatalog>,
    // Optional — the screenshot path has no player plugin, so
    // Stamina exists only with `--screenshot-ui`. Absent means
    // full (100): the stats panel shows the real value when the
    // player exists and never crashes when it does not.
    stamina: Option<Res<'w, Stamina>>,
    saves: Res<'w, SaveList>,
}

/// Gameplay options: (label, kind). Only two exist — the rest
/// of Skyrim's Gameplay settings (difficulty, autosave) need
/// systems that do not exist yet.
/// The Gameplay settings rows. Indices are load-bearing: the
/// checkbox helpers below map row 1 → Invert Y and row 2 → Real
/// HUD. Add a row here AND a case in
/// `gameplay_checkbox_value` / `toggle_gameplay_checkbox`.
const GAMEPLAY_OPTIONS: [(&str, OptionKind); 3] = [
    ("Look Sensitivity", OptionKind::Stepper),
    ("Invert Y", OptionKind::Checkbox),
    ("Real HUD", OptionKind::Checkbox),
];

/// The checkbox value for a Gameplay settings row (M4): row 1 is
/// Invert Y, row 2 is the real HUD. The render + click + keyboard
/// paths all go through this, so a new checkbox cannot silently
/// toggle the wrong setting (the pre-M4 code hardcoded `invert_y`
/// for every checkbox row).
fn gameplay_checkbox_value(settings: &GameSettings, index: usize) -> bool {
    match index {
        1 => settings.invert_y,
        2 => settings.ruffle_hud,
        _ => false,
    }
}

/// Toggle the checkbox on a Gameplay settings row (M4). See
/// `gameplay_checkbox_value` for the index mapping.
fn toggle_gameplay_checkbox(settings: &mut GameSettings, index: usize) {
    match index {
        1 => settings.invert_y = !settings.invert_y,
        2 => settings.ruffle_hud = !settings.ruffle_hud,
        _ => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionKind {
    Stepper,
    Checkbox,
}

/// Help topics: (title, body). Our own help text about the
/// engine's real controls and systems.
const HELP_TOPICS: [(&str, &str); 5] = [
    (
        "Controls",
        "Move with WASD or the joystick. Look with the mouse or by dragging. Space jumps, Shift sprints. E uses doors, talks to people, and picks things up. B opens your bag. Esc opens this menu.",
    ),
    (
        "Inventory",
        "Your bag holds what you pick up. E equips or uses the selected item, R drops it. The doll and the 3D item view show what you carry.",
    ),
    (
        "Dialogue",
        "Walk up to someone and press E to talk. Pick a topic to hear their answer. Tab closes. Topics are not gated by quests yet.",
    ),
    (
        "Doors",
        "Press E near a door to open it. Walk through an open doorway to travel. Closed doors block you.",
    ),
    (
        "Saving",
        "Save writes a file with your place and your pack. Load puts you back where you stood, with what you carried. The autosave slot keeps your last door crossing.",
    ),
];

/// The control list (read-only — remapping needs the input
/// system): (action, binding).
const CONTROLS: [(&str, &str); 8] = [
    ("Move", "WASD / Joystick"),
    ("Look", "Mouse / Drag"),
    ("Jump", "Space / Jump"),
    ("Sprint", "Shift / Sprint"),
    ("Use / Talk", "E / USE"),
    ("Inventory", "B / BAG"),
    ("Pause", "Esc / MENU"),
    ("Close menu", "Tab / Esc"),
];

/// Spawn one text node (absolute, stage coords).
#[allow(clippy::too_many_arguments)]
fn text_node(
    parent: &mut bevy::ecs::prelude::ChildSpawnerCommands,
    text: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    size: f32,
    color: bevy::color::Color,
    scale: f32,
) {
    parent.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(scale, x),
            top: px(scale, y),
            width: px(scale, w),
            height: px(scale, h),
            ..Default::default()
        },
        Text::new(text),
        TextFont {
            font_size: fs(scale, size),
            ..Default::default()
        },
        TextColor(color),
    ));
}

/// Build the whole pause UI from the current state. Called on
/// open and on every state change (via `PauseDirty`) — a full
/// rebuild, like the inventory/dialogue refresh.
fn build_pause_ui(commands: &mut Commands, p: &PauseUiParams) {
    let scale = p.scale.0;
    let state = &p.state;
    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                ..Default::default()
            },
            BackgroundColor(FADER_BG),
            PauseUiRoot,
        ))
        .with_children(|root| {
            // The big panel backdrop (966x552 at (157.1, 46.3)).
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: px(scale, PANEL_X),
                    top: px(scale, PANEL_Y),
                    width: px(scale, PANEL_W),
                    height: px(scale, PANEL_H),
                    border: UiRect::all(Val::Px(1.0)),
                    ..Default::default()
                },
                BackgroundColor(PANEL_BG),
                BorderColor::all(PANEL_BORDER),
            ));

            // Tabs: Quests / Stats / System.
            for tab in [PauseTab::Quests, PauseTab::Stats, PauseTab::System] {
                let (x, label) = tab.geometry();
                let selected = state.tab == tab;
                root.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(scale, x),
                        top: px(scale, TAB_Y),
                        width: px(scale, TAB_W),
                        height: px(scale, 32.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..Default::default()
                    },
                    PauseTabButton { tab },
                ))
                .with_children(|tab_node| {
                    tab_node.spawn((
                        Text::new(label),
                        TextFont {
                            font_size: fs(scale, TAB_FONT),
                            ..Default::default()
                        },
                        TextColor(if selected { GOLD } else { TEXT_DIM }),
                    ));
                })
                .observe(on_tab_click);
            }

            match state.tab {
                PauseTab::Quests => build_quests_content(root, p),
                PauseTab::Stats => build_stats_content(root, p),
                PauseTab::System => build_system_content(root, p),
            }

            build_bottom_bar(root, p);
        });
}

/// The bottom bar: band, date, [Tab] Close, level meter.
fn build_bottom_bar(root: &mut bevy::ecs::prelude::ChildSpawnerCommands, p: &PauseUiParams) {
    let scale = p.scale.0;
    // Band (the .swf's 60% grey/black gradient, flat here).
    root.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: px(scale, BOTTOM_BAR_Y),
            width: Val::Percent(100.0),
            height: px(scale, STAGE_HEIGHT - BOTTOM_BAR_Y),
            ..Default::default()
        },
        BackgroundColor(PANEL_BG),
    ));
    // Date (static start date — no time system yet).
    text_node(
        root,
        GAME_START_DATE,
        DATE_X,
        DATE_Y,
        300.0,
        28.0,
        DATE_FONT,
        TEXT_WHITE,
        scale,
    );
    // Close button. No key prompt: the .swf's button carries a
    // [Tab] key icon, but there is no Tab key on a phone — the
    // word is the tap target (v0.4.13).
    root.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(scale, BAR_BUTTONS_X),
            top: px(scale, BAR_BUTTONS_Y),
            width: px(scale, 150.0),
            height: px(scale, 28.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            ..Default::default()
        },
        PauseCloseButton,
    ))
    .with_children(|button| {
        button.spawn((
            Text::new("Close"),
            TextFont {
                font_size: fs(scale, BAR_BUTTON_FONT),
                ..Default::default()
            },
            TextColor(TEXT_WHITE),
        ));
    })
    .observe(on_close_click);
    // Level meter: "LEVEL" + number + (empty) progress bar.
    text_node(
        root,
        "LEVEL",
        LEVEL_LABEL_X,
        LEVEL_LABEL_Y,
        51.0,
        23.0,
        16.0,
        TEXT_WHITE,
        scale,
    );
    text_node(
        root,
        "1",
        LEVEL_NUM_X,
        LEVEL_NUM_Y,
        31.0,
        34.0,
        24.0,
        TEXT_WHITE,
        scale,
    );
    // Bar frame (220x22); the fill is empty — no XP system.
    root.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(scale, LEVEL_BAR_X),
            top: px(scale, LEVEL_BAR_Y),
            width: px(scale, LEVEL_BAR_W),
            height: px(scale, LEVEL_BAR_H),
            border: UiRect::all(Val::Px(1.0)),
            ..Default::default()
        },
        BackgroundColor(bevy::color::Color::srgba(0.0, 0.0, 0.0, 0.6)),
        BorderColor::all(bevy::color::Color::srgba(0.73, 0.74, 0.75, 0.6)),
    ));
}

/// Quests tab: the quest list (left), divider, title +
/// description + objectives (right).
fn build_quests_content(root: &mut bevy::ecs::prelude::ChildSpawnerCommands, p: &PauseUiParams) {
    let scale = p.scale.0;
    let state = &p.state;
    let selected = state.quest.min(p.index.entries.len().saturating_sub(1));

    // The quest list: 17 rows visible, centered scroll on the
    // selection (the inventory/dialogue pattern).
    let viewport_h = QUEST_VISIBLE_ROWS as f32 * QUEST_ROW_H;
    let content_h = p.index.entries.len() as f32 * QUEST_ROW_H;
    let scroll = crate::inventory::centered_scroll_y(
        selected as f32 * QUEST_ROW_H,
        (QUEST_VISIBLE_ROWS as f32 / 2.0 - 0.5) * QUEST_ROW_H,
        content_h,
        viewport_h,
    );
    root.spawn(Node {
        position_type: PositionType::Absolute,
        left: px(scale, QUEST_LIST_X),
        top: px(scale, QUEST_LIST_Y),
        width: px(scale, QUEST_ROW_W),
        height: px(scale, viewport_h),
        // Column: the default Row lays the 339 rows out
        // side-by-side across the panel (proven by the first
        // proof render — the quest list was a horizontal strip).
        flex_direction: FlexDirection::Column,
        overflow: Overflow::scroll_y(),
        ..Default::default()
    })
    .insert(ScrollPosition(bevy::math::Vec2::new(0.0, scroll * scale)))
    .with_children(|list| {
        for (i, entry) in p.index.entries.iter().enumerate() {
            let is_selected = i == selected;
            list.spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: px(scale, QUEST_ROW_H),
                    flex_shrink: 0.0,
                    align_items: AlignItems::Center,
                    // Clip: a long quest name runs past the
                    // 230px row — the clip keeps it inside
                    // (the game clips too; the font is the
                    // constraint, as in the inventory). No
                    // wrap: a wrapped name's second line
                    // spilled into the next row on the phone
                    // (v0.4.13: "East Empire Warehouse
                    // Dialogue and Handler" printed on two
                    // rows) — one line, clipped at the right.
                    overflow: Overflow::clip(),
                    ..Default::default()
                },
                PauseQuestRow { index: i },
            ))
            .with_children(|row| {
                row.spawn((
                    Text::new(entry.name.clone()),
                    TextFont {
                        font_size: fs(scale, QUEST_ROW_FONT),
                        ..Default::default()
                    },
                    TextColor(if is_selected { GOLD } else { TEXT_DIM }),
                    TextLayout::no_wrap(),
                ));
            })
            .observe(on_quest_click);
        }
    });

    // Divider (15x487, grey 50%).
    root.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(scale, QUEST_DIVIDER_X),
            top: px(scale, PAGE_Y),
            width: px(scale, 15.0),
            height: px(scale, 487.0),
            ..Default::default()
        },
        BackgroundColor(DIVIDER_COLOR),
    ));

    // The selected quest's title, description, objectives.
    if let Some(entry) = p.index.entries.get(selected) {
        // Title: one line, centered over the detail pane,
        // clipped. The .swf field is 220x32 at (682.7, 129.3),
        // but quest names run far wider than 220px in Bevy's
        // font — the narrow field wraps them to four lines
        // (proven by the first proof render). The title spans
        // the description width instead; the .swf's endpiece
        // brackets span ~500px, so a wide centered title is
        // the faithful reconstruction. 20px, no wrap: at 24px
        // the longest name ("East Empire Warehouse Dialogue
        // and Handler") wrapped and the clip ate "Handler"
        // (v0.4.13); at 20px it fits the 603px field on one
        // line.
        root.spawn(Node {
            position_type: PositionType::Absolute,
            left: px(scale, QUEST_DESC_X),
            top: px(scale, QUEST_TITLE_Y),
            width: px(scale, QUEST_DESC_W),
            height: px(scale, 32.0),
            overflow: Overflow::clip(),
            justify_content: JustifyContent::Center,
            ..Default::default()
        })
        .with_children(|title| {
            title.spawn((
                Text::new(entry.name.clone()),
                TextFont {
                    font_size: fs(scale, QUEST_TITLE_FONT),
                    ..Default::default()
                },
                TextColor(TEXT_WHITE),
                TextLayout::no_wrap(),
            ));
        });
        // Description: one line, clipped like the game (the
        // .swf field is 603x32; the clip parent keeps it out
        // of the objectives below).
        root.spawn(Node {
            position_type: PositionType::Absolute,
            left: px(scale, QUEST_DESC_X),
            top: px(scale, QUEST_DESC_Y),
            width: px(scale, QUEST_DESC_W),
            height: px(scale, 32.0),
            overflow: Overflow::clip(),
            ..Default::default()
        })
        .with_children(|desc| {
            desc.spawn((
                Text::new(entry.description.clone()),
                TextFont {
                    font_size: fs(scale, QUEST_DESC_FONT),
                    ..Default::default()
                },
                TextColor(TEXT_WHITE),
            ));
        });
        // Objectives header: rule + centered label.
        root.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(scale, OBJECTIVES_HEADER_X),
                top: px(scale, OBJECTIVES_HEADER_Y + 9.0),
                width: px(scale, 481.0),
                height: px(scale, 2.0),
                ..Default::default()
            },
            BackgroundColor(DIVIDER_COLOR),
        ));
        text_node(
            root,
            "Objectives",
            OBJECTIVES_HEADER_X + 181.0,
            OBJECTIVES_HEADER_Y + 2.0,
            122.0,
            32.0,
            24.0,
            TEXT_WHITE,
            scale,
        );
        // Objectives: first 10 visible (350px before the bottom
        // bar), with a "+N more" line when there are more.
        let visible = entry.objectives.iter().take(10);
        let hidden = entry.objectives.len().saturating_sub(10);
        for (i, objective) in visible.enumerate() {
            let y = OBJECTIVES_Y + i as f32 * OBJECTIVE_ROW_H;
            // Marker square (the .swf's 11x17 diamond).
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: px(scale, OBJECTIVES_X + 4.0),
                    top: px(scale, y + 11.0),
                    width: px(scale, OBJECTIVE_MARKER_SIZE),
                    height: px(scale, OBJECTIVE_MARKER_SIZE),
                    ..Default::default()
                },
                BackgroundColor(TEXT_WHITE),
            ));
            root.spawn(Node {
                position_type: PositionType::Absolute,
                left: px(scale, OBJECTIVES_X + 22.0),
                top: px(scale, y + 3.2),
                // 590 wide (the panel ends at 1123.1): at 16px
                // most objectives fit one line; no wrap, so a
                // long one clips at the right instead of
                // printing a half-cut second line (v0.4.13:
                // "Speak to Orthas Endario..." showed a sliver
                // of its second line under the first).
                width: px(scale, 590.0),
                height: px(scale, 32.0),
                overflow: Overflow::clip(),
                ..Default::default()
            })
            .with_children(|row| {
                row.spawn((
                    Text::new(objective.clone()),
                    TextFont {
                        font_size: fs(scale, OBJECTIVE_FONT),
                        ..Default::default()
                    },
                    TextColor(TEXT_WHITE),
                    TextLayout::no_wrap(),
                ));
            });
        }
        if hidden > 0 {
            text_node(
                root,
                &format!("+{hidden} more"),
                OBJECTIVES_X + 22.0,
                OBJECTIVES_Y + 10.0 * OBJECTIVE_ROW_H,
                200.0,
                32.0,
                OBJECTIVE_FONT,
                TEXT_DIM,
                scale,
            );
        }
    }
}

/// Stats tab: the General Stats category + the real values.
fn build_stats_content(root: &mut bevy::ecs::prelude::ChildSpawnerCommands, p: &PauseUiParams) {
    let scale = p.scale.0;
    // Category list: only General Stats — the other categories
    // need their tracking systems.
    text_node(
        root,
        "General Stats",
        STATS_CAT_X,
        STATS_CAT_Y,
        230.0,
        23.0,
        16.0,
        GOLD,
        scale,
    );
    // Divider.
    root.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(scale, STATS_DIVIDER_X),
            top: px(scale, PAGE_Y),
            width: px(scale, 15.0),
            height: px(scale, 487.0),
            ..Default::default()
        },
        BackgroundColor(DIVIDER_COLOR),
    ));
    // The stats we actually know.
    let weight = p.inventory.total_weight(&p.catalog);
    let rows: [(&str, String); 9] = [
        ("Level", "1".to_string()),
        ("Health", "100 / 100".to_string()),
        ("Magicka", "100 / 100".to_string()),
        (
            "Stamina",
            format!(
                "{} / 100",
                p.stamina.as_deref().map(|s| s.value).unwrap_or(100.0) as i32
            ),
        ),
        ("Gold", format!("{}", p.inventory.gold)),
        ("Carry Weight", format!("{weight:.0} / 300")),
        ("Days Passed", "0".to_string()),
        ("Quests Completed", "0".to_string()),
        ("Locations Discovered", "0".to_string()),
    ];
    for (i, (label, value)) in rows.iter().enumerate() {
        let y = STATS_LIST_Y + i as f32 * STAT_ROW_H;
        text_node(
            root,
            label,
            STATS_LIST_X + 25.0,
            y + 4.5,
            350.0,
            30.0,
            STAT_FONT,
            TEXT_WHITE,
            scale,
        );
        text_node(
            root,
            value,
            STATS_LIST_X + 25.0 + 352.0,
            y + 4.5,
            220.0,
            30.0,
            STAT_FONT,
            TEXT_WHITE,
            scale,
        );
    }
}

/// System tab: the category list (left) + the selected
/// category's panel (right).
fn build_system_content(root: &mut bevy::ecs::prelude::ChildSpawnerCommands, p: &PauseUiParams) {
    let scale = p.scale.0;
    let state = &p.state;

    // Category list: Settings / Controls / Load / Save / Help /
    // Quit, 25px pitch.
    for (i, system) in PauseSystem::ALL.iter().enumerate() {
        let is_selected = state.system == *system;
        let y = SYSTEM_CAT_Y + i as f32 * QUEST_ROW_H;
        root.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(scale, SYSTEM_CAT_X),
                top: px(scale, y),
                width: px(scale, QUEST_ROW_W),
                height: px(scale, QUEST_ROW_H),
                align_items: AlignItems::Center,
                ..Default::default()
            },
            PauseSystemRow { system: *system },
        ))
        .with_children(|row| {
            row.spawn((
                Text::new(system.label()),
                TextFont {
                    font_size: fs(scale, QUEST_ROW_FONT),
                    ..Default::default()
                },
                TextColor(if is_selected { GOLD } else { TEXT_DIM }),
            ));
        })
        .observe(on_system_row_click);
    }

    // Divider.
    root.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(scale, SYSTEM_DIVIDER_X),
            top: px(scale, PAGE_Y),
            width: px(scale, 15.0),
            height: px(scale, 487.0),
            ..Default::default()
        },
        BackgroundColor(DIVIDER_COLOR),
    ));

    // Version text (the engine version, from Cargo.toml).
    text_node(
        root,
        &format!("DovahEngine {}", env!("CARGO_PKG_VERSION")),
        VERSION_X,
        VERSION_Y,
        200.0,
        18.0,
        12.0,
        TEXT_DIM,
        scale,
    );

    match state.system {
        PauseSystem::Settings => build_settings_panel(root, p),
        PauseSystem::Controls => build_controls_panel(root, p),
        PauseSystem::Load => build_save_load_panel(root, p, true),
        PauseSystem::Save => build_save_load_panel(root, p, false),
        PauseSystem::Help => build_help_panel(root, p),
        PauseSystem::Quit => build_quit_panel(root, p),
    }
}

/// Settings panel: the category list, or the selected
/// category's options.
fn build_settings_panel(root: &mut bevy::ecs::prelude::ChildSpawnerCommands, p: &PauseUiParams) {
    let scale = p.scale.0;
    let state = &p.state;
    match state.settings_view {
        SettingsView::Categories => {
            for (i, category) in SettingsCategory::ALL.iter().enumerate() {
                let is_selected = state.settings_category == *category;
                let y = SETTINGS_PANEL_Y + i as f32 * SETTINGS_CAT_ROW_H;
                root.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(scale, SYSTEM_PANEL_X),
                        top: px(scale, y),
                        width: px(scale, 200.0),
                        height: px(scale, SETTINGS_CAT_ROW_H),
                        align_items: AlignItems::Center,
                        ..Default::default()
                    },
                    PauseSettingsCategoryRow {
                        category: *category,
                    },
                ))
                .with_children(|row| {
                    row.spawn((
                        Text::new(category.label()),
                        TextFont {
                            font_size: fs(scale, SETTINGS_CAT_FONT),
                            ..Default::default()
                        },
                        TextColor(if is_selected { GOLD } else { TEXT_DIM }),
                    ));
                })
                .observe(on_settings_category_click);
            }
        }
        SettingsView::Options => match state.settings_category {
            SettingsCategory::Gameplay => {
                for (i, (label, kind)) in GAMEPLAY_OPTIONS.iter().enumerate() {
                    let is_selected = state.setting == i;
                    let y = OPTIONS_PANEL_Y + i as f32 * OPTION_ROW_H;
                    text_node(
                        root,
                        label,
                        SYSTEM_PANEL_X + 2.0,
                        y + 7.0,
                        200.0,
                        30.0,
                        OPTION_LABEL_FONT,
                        if is_selected { GOLD } else { TEXT_DIM },
                        scale,
                    );
                    match kind {
                        OptionKind::Stepper => {
                            // "< 100% >" — the .swf stepper
                            // (prev arrow, value, next arrow).
                            text_node(
                                root,
                                &format!("< {} >", p.settings.sensitivity.label()),
                                SYSTEM_PANEL_X + OPTION_STEPPER_X,
                                y + 5.0,
                                190.0,
                                29.0,
                                OPTION_VALUE_FONT,
                                TEXT_WHITE,
                                scale,
                            );
                        }
                        OptionKind::Checkbox => {
                            text_node(
                                root,
                                if gameplay_checkbox_value(&p.settings, i) {
                                    "[X]"
                                } else {
                                    "[ ]"
                                },
                                SYSTEM_PANEL_X + OPTION_CHECKBOX_X,
                                y + 7.0,
                                40.0,
                                30.0,
                                OPTION_LABEL_FONT,
                                TEXT_WHITE,
                                scale,
                            );
                        }
                    }
                    // The whole row is clickable (step / toggle).
                    root.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(scale, SYSTEM_PANEL_X),
                            top: px(scale, y),
                            width: px(scale, 410.0),
                            height: px(scale, OPTION_ROW_H),
                            ..Default::default()
                        },
                        PauseOptionRow { index: i },
                    ))
                    .observe(on_option_click);
                }
            }
            SettingsCategory::Display => {
                // No display system exists to configure — say so,
                // do not draw fake options.
                text_node(
                    root,
                    "No display settings yet.",
                    SYSTEM_PANEL_X,
                    OPTIONS_PANEL_Y,
                    500.0,
                    30.0,
                    OPTION_LABEL_FONT,
                    TEXT_DIM,
                    scale,
                );
                text_node(
                    root,
                    "The display system is a later milestone.",
                    SYSTEM_PANEL_X,
                    OPTIONS_PANEL_Y + OPTION_ROW_H,
                    500.0,
                    30.0,
                    OPTION_LABEL_FONT,
                    TEXT_DIM,
                    scale,
                );
            }
            SettingsCategory::Audio => {
                // No audio system exists to configure.
                text_node(
                    root,
                    "No audio settings yet.",
                    SYSTEM_PANEL_X,
                    OPTIONS_PANEL_Y,
                    500.0,
                    30.0,
                    OPTION_LABEL_FONT,
                    TEXT_DIM,
                    scale,
                );
                text_node(
                    root,
                    "There is no sound system yet.",
                    SYSTEM_PANEL_X,
                    OPTIONS_PANEL_Y + OPTION_ROW_H,
                    500.0,
                    30.0,
                    OPTION_LABEL_FONT,
                    TEXT_DIM,
                    scale,
                );
            }
        },
    }
}

/// Controls panel: the real bindings, read-only (remapping
/// needs the input system).
fn build_controls_panel(root: &mut bevy::ecs::prelude::ChildSpawnerCommands, p: &PauseUiParams) {
    let scale = p.scale.0;
    for (i, (action, binding)) in CONTROLS.iter().enumerate() {
        let y = SETTINGS_PANEL_Y + i as f32 * STAT_ROW_H;
        text_node(
            root,
            action,
            SYSTEM_PANEL_X + 2.0,
            y + 4.5,
            250.0,
            30.0,
            STAT_FONT,
            TEXT_WHITE,
            scale,
        );
        text_node(
            root,
            binding,
            SYSTEM_PANEL_X + 252.0,
            y + 4.5,
            300.0,
            30.0,
            STAT_FONT,
            TEXT_WHITE,
            scale,
        );
    }
}

/// Load / Save panel: the save list (11 rows at 27px pitch),
/// the selected save's detail, and — on Load — the honest
/// message that loading needs the scene-reload hook.
fn build_save_load_panel(
    root: &mut bevy::ecs::prelude::ChildSpawnerCommands,
    p: &PauseUiParams,
    load: bool,
) {
    let scale = p.scale.0;
    let state = &p.state;
    let selected = state.save.min(p.saves.saves.len().saturating_sub(1));

    if p.saves.saves.is_empty() {
        text_node(
            root,
            "No saved games yet.",
            SYSTEM_PANEL_X,
            SETTINGS_PANEL_Y,
            300.0,
            30.0,
            SAVE_FONT,
            TEXT_DIM,
            scale,
        );
    } else {
        let viewport_h = SAVE_VISIBLE_ROWS as f32 * SAVE_ROW_H;
        let content_h = p.saves.saves.len() as f32 * SAVE_ROW_H;
        let scroll = crate::inventory::centered_scroll_y(
            selected as f32 * SAVE_ROW_H,
            (SAVE_VISIBLE_ROWS as f32 / 2.0 - 0.5) * SAVE_ROW_H,
            content_h,
            viewport_h,
        );
        root.spawn(Node {
            position_type: PositionType::Absolute,
            left: px(scale, SYSTEM_PANEL_X),
            top: px(scale, SETTINGS_PANEL_Y),
            width: px(scale, 340.0),
            height: px(scale, viewport_h),
            // Flex defaults to Row — without this the saves lay
            // out horizontally, and save 2 lands on the detail
            // panel (the UI M1 quest-list bug, again).
            flex_direction: FlexDirection::Column,
            overflow: Overflow::scroll_y(),
            ..Default::default()
        })
        .insert(ScrollPosition(bevy::math::Vec2::new(0.0, scroll * scale)))
        .with_children(|list| {
            for (i, save) in p.saves.saves.iter().enumerate() {
                let is_selected = i == selected;
                list.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: px(scale, SAVE_ROW_H),
                        flex_shrink: 0.0,
                        align_items: AlignItems::Center,
                        ..Default::default()
                    },
                    PauseSaveRow { index: i },
                ))
                .with_children(|row| {
                    // SaveNumber (the .swf's 40-48px field at
                    // x -6..2) then the name at x 52.
                    row.spawn(Node {
                        width: px(scale, 52.0),
                        flex_shrink: 0.0,
                        ..Default::default()
                    })
                    .with_children(|num| {
                        num.spawn((
                            Text::new(format!("{}", i + 1)),
                            TextFont {
                                font_size: fs(scale, SAVE_FONT),
                                ..Default::default()
                            },
                            TextColor(if is_selected { GOLD } else { TEXT_DIM }),
                        ));
                    });
                    row.spawn((
                        Text::new(save.location.clone()),
                        TextFont {
                            font_size: fs(scale, SAVE_FONT),
                            ..Default::default()
                        },
                        TextColor(if is_selected { GOLD } else { TEXT_DIM }),
                    ));
                })
                .observe(on_save_row_click);
            }
        });

        // The selected save's detail (the .swf's detail panel
        // at (340, 0) of the SaveLoadPanel): level + date.
        if let Some(save) = p.saves.saves.get(selected) {
            text_node(
                root,
                &format!("Level {}", save.level),
                SYSTEM_PANEL_X + 340.0,
                SETTINGS_PANEL_Y,
                200.0,
                30.0,
                SAVE_FONT,
                TEXT_WHITE,
                scale,
            );
            text_node(
                root,
                &save.saved_at_display,
                SYSTEM_PANEL_X + 340.0,
                SETTINGS_PANEL_Y + SAVE_ROW_H,
                400.0,
                30.0,
                SAVE_FONT,
                TEXT_WHITE,
                scale,
            );
            text_node(
                root,
                &save.game_date,
                SYSTEM_PANEL_X + 340.0,
                SETTINGS_PANEL_Y + 2.0 * SAVE_ROW_H,
                400.0,
                30.0,
                SAVE_FONT,
                TEXT_DIM,
                scale,
            );
        }
    }

    if load {
        // [E] Load Game button, below the list (mirrors the
        // Save button; hidden when there is nothing to load).
        if !p.saves.saves.is_empty() {
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: px(scale, SYSTEM_PANEL_X),
                    top: px(
                        scale,
                        SETTINGS_PANEL_Y + SAVE_VISIBLE_ROWS as f32 * SAVE_ROW_H + 13.0,
                    ),
                    width: px(scale, 200.0),
                    height: px(scale, 30.0),
                    align_items: AlignItems::Center,
                    ..Default::default()
                },
                PauseLoadButton,
            ))
            .with_children(|button| {
                button.spawn((
                    Text::new("[E] Load Game"),
                    TextFont {
                        font_size: fs(scale, 20.0),
                        ..Default::default()
                    },
                    TextColor(GOLD),
                ));
            })
            .observe(on_load_button_click);
        }
    } else {
        // [E] Save Game button, below the list.
        root.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(scale, SYSTEM_PANEL_X),
                top: px(
                    scale,
                    SETTINGS_PANEL_Y + SAVE_VISIBLE_ROWS as f32 * SAVE_ROW_H + 13.0,
                ),
                width: px(scale, 200.0),
                height: px(scale, 30.0),
                align_items: AlignItems::Center,
                ..Default::default()
            },
            PauseSaveButton,
        ))
        .with_children(|button| {
            button.spawn((
                Text::new("[E] Save Game"),
                TextFont {
                    font_size: fs(scale, 20.0),
                    ..Default::default()
                },
                TextColor(GOLD),
            ));
        })
        .observe(on_save_button_click);
    }
}

/// Help panel: the topic list + the selected topic's text.
fn build_help_panel(root: &mut bevy::ecs::prelude::ChildSpawnerCommands, p: &PauseUiParams) {
    let scale = p.scale.0;
    let state = &p.state;
    for (i, (title, _)) in HELP_TOPICS.iter().enumerate() {
        let is_selected = state.help == i;
        let y = SETTINGS_PANEL_Y + i as f32 * SETTINGS_CAT_ROW_H;
        root.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(scale, SYSTEM_PANEL_X),
                top: px(scale, y),
                width: px(scale, 200.0),
                height: px(scale, SETTINGS_CAT_ROW_H),
                align_items: AlignItems::Center,
                ..Default::default()
            },
            PauseHelpRow { index: i },
        ))
        .with_children(|row| {
            row.spawn((
                Text::new(*title),
                TextFont {
                    font_size: fs(scale, SETTINGS_CAT_FONT),
                    ..Default::default()
                },
                TextColor(if is_selected { GOLD } else { TEXT_DIM }),
            ));
        })
        .observe(on_help_click);
    }
    if let Some((_, body)) = HELP_TOPICS.get(state.help) {
        root.spawn(Node {
            position_type: PositionType::Absolute,
            left: px(scale, SYSTEM_PANEL_X + 270.0),
            top: px(scale, SETTINGS_PANEL_Y),
            width: px(scale, 540.0),
            height: px(scale, 300.0),
            ..Default::default()
        })
        .with_children(|text_wrap| {
            text_wrap.spawn((
                Text::new(*body),
                TextFont {
                    font_size: fs(scale, 20.0),
                    ..Default::default()
                },
                TextColor(TEXT_WHITE),
            ));
        });
    }
}

/// Quit panel: Quit to Desktop / Cancel (Cancel is the safe
/// default — `PauseState::default` selects it).
fn build_quit_panel(root: &mut bevy::ecs::prelude::ChildSpawnerCommands, p: &PauseUiParams) {
    let scale = p.scale.0;
    let state = &p.state;
    for (i, label) in ["Quit to Desktop", "Cancel"].iter().enumerate() {
        let is_selected = state.quit == i;
        let y = QUIT_PANEL_Y + i as f32 * QUIT_ROW_H;
        root.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(scale, SYSTEM_PANEL_X),
                top: px(scale, y),
                width: px(scale, 185.0),
                height: px(scale, QUIT_ROW_H),
                align_items: AlignItems::Center,
                ..Default::default()
            },
            PauseQuitRow { index: i },
        ))
        .with_children(|row| {
            row.spawn((
                Text::new(*label),
                TextFont {
                    font_size: fs(scale, QUIT_FONT),
                    ..Default::default()
                },
                TextColor(if is_selected { GOLD } else { TEXT_DIM }),
            ));
        })
        .observe(on_quit_click);
    }
}

// ---------------------------------------------------------------------------
// Click observers (touch + mouse — the inventory/dialogue
// pattern: the observer mutates selection state and sets
// `PauseDirty`; `refresh_pause_ui` rebuilds)
// ---------------------------------------------------------------------------

fn on_tab_click(
    event: On<PointerClick>,
    tabs: Query<&PauseTabButton>,
    mut state: ResMut<PauseState>,
    mut dirty: ResMut<PauseDirty>,
) {
    let Ok(tab) = tabs.get(event.event_target()) else {
        return;
    };
    if state.tab != tab.tab {
        state.tab = tab.tab;
        state.settings_view = SettingsView::Categories;
        dirty.0 = true;
    }
}

fn on_quest_click(
    event: On<PointerClick>,
    rows: Query<&PauseQuestRow>,
    mut state: ResMut<PauseState>,
    mut dirty: ResMut<PauseDirty>,
) {
    let Ok(row) = rows.get(event.event_target()) else {
        return;
    };
    if state.quest != row.index {
        state.quest = row.index;
        dirty.0 = true;
    }
}

fn on_system_row_click(
    event: On<PointerClick>,
    rows: Query<&PauseSystemRow>,
    mut state: ResMut<PauseState>,
    mut dirty: ResMut<PauseDirty>,
) {
    let Ok(row) = rows.get(event.event_target()) else {
        return;
    };
    if state.system != row.system {
        state.system = row.system;
        state.settings_view = SettingsView::Categories;
        dirty.0 = true;
    }
}

fn on_settings_category_click(
    event: On<PointerClick>,
    rows: Query<&PauseSettingsCategoryRow>,
    mut state: ResMut<PauseState>,
    mut dirty: ResMut<PauseDirty>,
) {
    let Ok(row) = rows.get(event.event_target()) else {
        return;
    };
    // A tap selects the category AND opens its options (the
    // game's click behavior); the keyboard splits the two.
    state.settings_category = row.category;
    state.settings_view = SettingsView::Options;
    state.setting = 0;
    dirty.0 = true;
}

fn on_option_click(
    event: On<PointerClick>,
    rows: Query<&PauseOptionRow>,
    mut state: ResMut<PauseState>,
    mut settings: ResMut<GameSettings>,
    mut dirty: ResMut<PauseDirty>,
) {
    let Ok(row) = rows.get(event.event_target()) else {
        return;
    };
    state.setting = row.index;
    // Tap = activate: the stepper steps forward, the checkbox
    // toggles.
    match GAMEPLAY_OPTIONS[row.index].1 {
        OptionKind::Stepper => settings.sensitivity = settings.sensitivity.next(),
        OptionKind::Checkbox => toggle_gameplay_checkbox(&mut settings, row.index),
    }
    dirty.0 = true;
}

fn on_save_row_click(
    event: On<PointerClick>,
    rows: Query<&PauseSaveRow>,
    mut state: ResMut<PauseState>,
    mut dirty: ResMut<PauseDirty>,
) {
    let Ok(row) = rows.get(event.event_target()) else {
        return;
    };
    if state.save != row.index {
        state.save = row.index;
        dirty.0 = true;
    }
}

fn on_quit_click(
    event: On<PointerClick>,
    rows: Query<&PauseQuitRow>,
    mut state: ResMut<PauseState>,
    mut dirty: ResMut<PauseDirty>,
    mut exit: bevy::ecs::message::MessageWriter<bevy::app::AppExit>,
) {
    let Ok(row) = rows.get(event.event_target()) else {
        return;
    };
    state.quit = row.index;
    if row.index == 0 {
        exit.write(bevy::app::AppExit::Success);
    } else {
        // Cancel: back to the Settings category.
        state.system = PauseSystem::Settings;
        dirty.0 = true;
    }
}

fn on_help_click(
    event: On<PointerClick>,
    rows: Query<&PauseHelpRow>,
    mut state: ResMut<PauseState>,
    mut dirty: ResMut<PauseDirty>,
) {
    let Ok(row) = rows.get(event.event_target()) else {
        return;
    };
    if state.help != row.index {
        state.help = row.index;
        dirty.0 = true;
    }
}

fn on_close_click(
    _event: On<PointerClick>,
    mut commands: Commands,
    ui_roots: Query<Entity, With<PauseUiRoot>>,
    mut open: ResMut<PauseOpen>,
    mut dirty: ResMut<PauseDirty>,
) {
    close_pause_ui(&mut commands, &ui_roots, &mut open, &mut dirty);
}

fn on_save_button_click(_event: On<PointerClick>, mut requested: ResMut<SaveRequested>) {
    requested.0 = true;
}

/// The Load Game button: request the selected save, then close
/// the pause UI — the load despawns the scene, not the pause
/// root, so the UI must go away at request time.
#[allow(clippy::too_many_arguments)]
fn on_load_button_click(
    _event: On<PointerClick>,
    mut commands: Commands,
    ui_roots: Query<Entity, With<PauseUiRoot>>,
    mut open: ResMut<PauseOpen>,
    mut dirty: ResMut<PauseDirty>,
    state: Res<PauseState>,
    saves: Res<SaveList>,
    mut requested: ResMut<crate::save::LoadRequested>,
) {
    let selected = state.save.min(saves.saves.len().saturating_sub(1));
    if let Some(save) = saves.saves.get(selected) {
        requested.0 = Some(save.path.clone());
        close_pause_ui(&mut commands, &ui_roots, &mut open, &mut dirty);
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// The inventory/dialogue open state LAST frame — the Esc
/// toggle opens the pause only when both were closed last
/// frame AND this frame, so an Esc that closes the inventory
/// (or dialogue) never also opens the pause in the same frame
/// (system order between the menu plugins is unspecified).
#[derive(Debug, Default, Resource)]
struct PrevMenus {
    inventory: bool,
    dialogue: bool,
}

/// Close the pause UI: despawn the root, clear the flags.
fn close_pause_ui(
    commands: &mut Commands,
    ui_roots: &Query<Entity, With<PauseUiRoot>>,
    open: &mut PauseOpen,
    dirty: &mut PauseDirty,
) {
    for entity in ui_roots.iter() {
        commands.entity(entity).despawn();
    }
    open.0 = false;
    dirty.0 = false;
}

/// Rebuild the UI when the state changed since the last build
/// (or the menu just opened). Runs every frame; cheap when
/// nothing is dirty.
fn refresh_pause_ui(
    mut commands: Commands,
    ui_roots: Query<Entity, With<PauseUiRoot>>,
    params: PauseUiParams,
    open: Res<PauseOpen>,
    mut dirty: ResMut<PauseDirty>,
) {
    if !open.0 || !dirty.0 {
        return;
    }
    for entity in ui_roots.iter() {
        commands.entity(entity).despawn();
    }
    build_pause_ui(&mut commands, &params);
    dirty.0 = false;
}

/// The pause keys + the MENU touch button. When closed: Esc
/// (or MENU) opens, gated on the other menus being closed last
/// frame and this frame (see `PrevMenus`). When open: Esc/Tab
/// closes (or backs out of the Settings options), Left/Right
/// switch tabs (or step the sensitivity), Up/Down navigate,
/// E activates.
#[allow(clippy::too_many_arguments)]
fn pause_keys(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    touch: Option<Res<crate::touch::TouchInputState>>,
    inventory_open: Option<Res<crate::inventory::InventoryOpen>>,
    dialogue_open: Option<Res<crate::dialogue::DialogueOpen>>,
    mut prev: ResMut<PrevMenus>,
    mut open: ResMut<PauseOpen>,
    mut state: ResMut<PauseState>,
    mut dirty: ResMut<PauseDirty>,
    mut settings: ResMut<GameSettings>,
    mut requested: ResMut<SaveRequested>,
    mut exit: bevy::ecs::message::MessageWriter<bevy::app::AppExit>,
    index: Res<QuestIndex>,
    save_dir: Option<Res<SaveDir>>,
    mut save_list: ResMut<SaveList>,
    ui_roots: Query<Entity, With<PauseUiRoot>>,
) {
    let cur_inv = inventory_open.map(|o| o.0).unwrap_or(false);
    let cur_dlg = dialogue_open.map(|o| o.0).unwrap_or(false);
    if !open.0 {
        let esc = keyboard.just_pressed(KeyCode::Escape);
        let menu = touch.as_deref().map(|t| t.menu_pressed).unwrap_or(false);
        if (esc || menu) && !prev.inventory && !prev.dialogue && !cur_inv && !cur_dlg {
            open.0 = true;
            dirty.0 = true;
            // Fresh save list for the Load/Save panels.
            if let Some(dir) = &save_dir {
                save_list.saves = list_saves(&dir.0);
            }
        }
        prev.inventory = cur_inv;
        prev.dialogue = cur_dlg;
        return;
    }
    prev.inventory = cur_inv;
    prev.dialogue = cur_dlg;

    // --- open: Esc/Tab closes (or backs out of Settings
    // options) ---
    if keyboard.just_pressed(KeyCode::Escape) || keyboard.just_pressed(KeyCode::Tab) {
        if state.tab == PauseTab::System
            && state.system == PauseSystem::Settings
            && state.settings_view == SettingsView::Options
        {
            state.settings_view = SettingsView::Categories;
            dirty.0 = true;
        } else {
            close_pause_ui(&mut commands, &ui_roots, &mut open, &mut dirty);
        }
        return;
    }

    // --- Left/Right: the sensitivity stepper, or tab switch ---
    let left = keyboard.just_pressed(KeyCode::ArrowLeft);
    let right = keyboard.just_pressed(KeyCode::ArrowRight);
    if left || right {
        let on_stepper = state.tab == PauseTab::System
            && state.system == PauseSystem::Settings
            && state.settings_view == SettingsView::Options
            && state.settings_category == SettingsCategory::Gameplay
            && state.setting == 0;
        if on_stepper {
            settings.sensitivity = if left {
                settings.sensitivity.prev()
            } else {
                settings.sensitivity.next()
            };
            dirty.0 = true;
            return;
        }
        let tabs = [PauseTab::Quests, PauseTab::Stats, PauseTab::System];
        let current = tabs.iter().position(|t| *t == state.tab).unwrap_or(0);
        let next = if left {
            (current + tabs.len() - 1) % tabs.len()
        } else {
            (current + 1) % tabs.len()
        };
        state.tab = tabs[next];
        state.settings_view = SettingsView::Categories;
        dirty.0 = true;
        return;
    }

    // --- Up/Down: navigate the current list (wraps) ---
    if keyboard.just_pressed(KeyCode::ArrowUp) || keyboard.just_pressed(KeyCode::ArrowDown) {
        let up = keyboard.just_pressed(KeyCode::ArrowUp);
        let step = |current: usize, len: usize| -> usize {
            if len == 0 {
                return 0;
            }
            if up {
                (current + len - 1) % len
            } else {
                (current + 1) % len
            }
        };
        match state.tab {
            PauseTab::Quests => {
                state.quest = step(state.quest, index.entries.len());
            }
            PauseTab::Stats => {}
            PauseTab::System => match state.system {
                PauseSystem::Settings => match state.settings_view {
                    SettingsView::Categories => {
                        let i = SettingsCategory::ALL
                            .iter()
                            .position(|c| *c == state.settings_category)
                            .unwrap_or(0);
                        state.settings_category =
                            SettingsCategory::ALL[step(i, SettingsCategory::ALL.len())];
                    }
                    SettingsView::Options => {
                        if state.settings_category == SettingsCategory::Gameplay {
                            state.setting = step(state.setting, GAMEPLAY_OPTIONS.len());
                        }
                    }
                },
                PauseSystem::Controls | PauseSystem::Save => {
                    // No panel list — Up/Down moves the
                    // System category list itself.
                    let i = PauseSystem::ALL
                        .iter()
                        .position(|s| *s == state.system)
                        .unwrap_or(0);
                    state.system = PauseSystem::ALL[step(i, PauseSystem::ALL.len())];
                    state.settings_view = SettingsView::Categories;
                }
                PauseSystem::Load => {
                    state.save = step(state.save, save_list.saves.len());
                }
                PauseSystem::Help => {
                    state.help = step(state.help, HELP_TOPICS.len());
                }
                PauseSystem::Quit => {
                    state.quit = step(state.quit, 2);
                }
            },
        }
        dirty.0 = true;
        return;
    }

    // --- E: activate ---
    if keyboard.just_pressed(KeyCode::KeyE) {
        match state.tab {
            PauseTab::Quests | PauseTab::Stats => {}
            PauseTab::System => match state.system {
                PauseSystem::Settings => match state.settings_view {
                    SettingsView::Categories => {
                        state.settings_view = SettingsView::Options;
                        state.setting = 0;
                        dirty.0 = true;
                    }
                    SettingsView::Options => {
                        if state.settings_category == SettingsCategory::Gameplay {
                            match GAMEPLAY_OPTIONS[state.setting].1 {
                                OptionKind::Stepper => {
                                    settings.sensitivity = settings.sensitivity.next();
                                }
                                OptionKind::Checkbox => {
                                    toggle_gameplay_checkbox(&mut settings, state.setting);
                                }
                            }
                            dirty.0 = true;
                        }
                    }
                },
                PauseSystem::Controls => {}
                PauseSystem::Load => {
                    // Request the load through state (this
                    // system is at Bevy's 16-parameter limit
                    // and cannot take LoadRequested itself) and
                    // close the pause — the load despawns the
                    // scene, not the pause root, so the UI must
                    // go away at request time.
                    if !save_list.saves.is_empty() {
                        state.load_requested = true;
                        close_pause_ui(&mut commands, &ui_roots, &mut open, &mut dirty);
                    }
                }
                PauseSystem::Save => {
                    requested.0 = true;
                }
                PauseSystem::Help => {}
                PauseSystem::Quit => {
                    if state.quit == 0 {
                        exit.write(bevy::app::AppExit::Success);
                    } else {
                        state.system = PauseSystem::Settings;
                        dirty.0 = true;
                    }
                }
            },
        }
    }
}

/// Load persisted settings at startup (best-effort — the
/// defaults stand when there is no file yet).
fn load_settings_at_startup(save_dir: Option<Res<SaveDir>>, mut settings: ResMut<GameSettings>) {
    if let Some(dir) = save_dir {
        *settings = load_settings(&dir.0);
    }
}

/// Persist settings when they change (best-effort).
fn persist_settings(save_dir: Option<Res<SaveDir>>, settings: Res<GameSettings>) {
    if settings.is_changed() {
        if let Some(dir) = save_dir {
            save_settings(&dir.0, &settings);
        }
    }
}

/// Apply the screenshot seed: open the pause on the requested
/// tab / System category (and Settings options when asked).
/// Also sets the menu scale from the capture target (no window
/// exists on the screenshot path) — the inventory seed's
/// pattern. Nine parameters because a seed touches nine pieces
/// of app state; the inventory seed takes fourteen.
#[allow(clippy::too_many_arguments)]
pub fn apply_pause_seed(
    seed: Option<Res<PauseSeed>>,
    mut state: ResMut<PauseState>,
    mut open: ResMut<PauseOpen>,
    mut dirty: ResMut<PauseDirty>,
    mut save_list: ResMut<SaveList>,
    save_dir: Option<Res<SaveDir>>,
    mut scale: ResMut<UiScale>,
    images: Res<bevy::asset::Assets<bevy::image::Image>>,
    screenshot_target: Option<Res<crate::screenshot::ScreenshotTarget>>,
) {
    let Some(seed) = seed else {
        return;
    };
    if let Some(target) = screenshot_target.as_deref() {
        if let Some(image) = images.get(&target.image) {
            scale.0 = image.height() as f32 / STAGE_HEIGHT;
        }
    }
    state.tab = seed.tab;
    state.system = seed.system;
    if let Some(category) = seed.settings_options {
        state.settings_category = category;
        state.settings_view = SettingsView::Options;
    }
    if let Some(dir) = &save_dir {
        save_list.saves = list_saves(&dir.0);
    }
    open.0 = true;
    dirty.0 = true;
}

/// Consume a cross-menu request to open the pause screen (UI
/// milestone 2 — the map tab's Journal button). The request
/// opens the pause on its current tab (Quests by default).
fn apply_open_pause_request(
    mut request: ResMut<OpenPauseRequest>,
    mut open: ResMut<PauseOpen>,
    mut dirty: ResMut<PauseDirty>,
) {
    if !request.0 {
        return;
    }
    request.0 = false;
    if !open.0 {
        open.0 = true;
        dirty.0 = true;
    }
}

/// Run condition: the character menu is closed — MENU/Esc must
/// not open the pause screen over it (UI milestone 2).
/// `pause_keys` is at Bevy's 16-parameter limit, so the gate is
/// a run condition, not a 17th parameter (the inventory's
/// `pause_closed` pattern). Safe: the character menu cannot
/// open while the pause screen is open, so gating `pause_keys`
/// never strands an open pause screen.
fn character_closed(character_open: Option<Res<crate::character::CharacterMenuOpen>>) -> bool {
    !character_open.map(|o| o.0).unwrap_or(false)
}

pub struct PausePlugin;

/// Press log for the MENU button: the pause screen opened or
/// closed this frame because of a MENU press. Runs after
/// `pause_keys` + `apply_open_pause_request` in the Update
/// tuple, so it sees the transition in the frame it happens.
/// (pause_keys is at Bevy's 16-parameter limit, so the outcome
/// is observed here instead of logged there — a keyboard Esc
/// press or a cross-menu request finds no pending MENU press
/// and logs nothing; a MENU press blocked by an open menu never
/// flips the state, so the flush logs it bare.)
fn log_menu_press_outcome(
    open: Res<PauseOpen>,
    mut press_log: Option<ResMut<crate::touch::PressLog>>,
) {
    if !open.is_changed() {
        return;
    }
    let Some(log) = press_log.as_deref_mut() else {
        return;
    };
    if let Some(count) = log.claim(crate::touch::PressButton::Menu) {
        let outcome = if open.0 { "pause opened" } else { "pause closed" };
        crate::startup_log::log(&crate::touch::PressLog::format_line(
            crate::touch::PressButton::Menu,
            count,
            Some(outcome),
        ));
    }
}

impl Plugin for PausePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PauseOpen>()
            .init_resource::<PauseState>()
            .init_resource::<PauseDirty>()
            .init_resource::<QuestIndex>()
            .init_resource::<GameSettings>()
            .init_resource::<PrevMenus>()
            .init_resource::<OpenPauseRequest>()
            .add_systems(Startup, load_settings_at_startup)
            .add_systems(
                Update,
                (
                    pause_keys.run_if(character_closed),
                    apply_open_pause_request,
                    log_menu_press_outcome,
                    refresh_pause_ui,
                    persist_settings,
                ),
            );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dovah-pause-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sensitivity_steps_and_multipliers() {
        assert_eq!(Sensitivity::default(), Sensitivity::Normal);
        // Steps wrap (the .swf stepper cycles).
        assert_eq!(Sensitivity::Normal.next(), Sensitivity::OneHalf);
        assert_eq!(Sensitivity::OneHalf.next(), Sensitivity::Double);
        assert_eq!(Sensitivity::Double.next(), Sensitivity::Half);
        assert_eq!(Sensitivity::Normal.prev(), Sensitivity::ThreeQuarters);
        assert_eq!(Sensitivity::ThreeQuarters.prev(), Sensitivity::Half);
        assert_eq!(Sensitivity::Half.prev(), Sensitivity::Double);
        assert!((Sensitivity::Half.multiplier() - 0.5).abs() < 0.001);
        assert!((Sensitivity::ThreeQuarters.multiplier() - 0.75).abs() < 0.001);
        assert!((Sensitivity::Normal.multiplier() - 1.0).abs() < 0.001);
        assert!((Sensitivity::OneHalf.multiplier() - 1.5).abs() < 0.001);
        assert!((Sensitivity::Double.multiplier() - 2.0).abs() < 0.001);
        assert_eq!(Sensitivity::Normal.label(), "100%");
        assert_eq!(Sensitivity::Half.label(), "50%");
    }

    #[test]
    fn settings_persist_round_trip() {
        let dir = test_dir("settings");
        let settings = GameSettings {
            sensitivity: Sensitivity::OneHalf,
            invert_y: true,
            ruffle_hud: false,
        };
        save_settings(&dir, &settings);
        let back = load_settings(&dir);
        assert_eq!(back.sensitivity, Sensitivity::OneHalf);
        assert!(back.invert_y);
        assert!(!back.ruffle_hud);
        // No file -> defaults.
        let _ = std::fs::remove_file(dir.join("settings.json"));
        let defaults = load_settings(&dir);
        assert_eq!(defaults, GameSettings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tab_and_system_names_parse() {
        assert_eq!(PauseTab::from_name("quests"), Some(PauseTab::Quests));
        assert_eq!(PauseTab::from_name("stats"), Some(PauseTab::Stats));
        assert_eq!(PauseTab::from_name("system"), Some(PauseTab::System));
        assert_eq!(PauseTab::from_name("settings"), Some(PauseTab::System));
        assert_eq!(
            PauseSystem::from_name("settings"),
            Some(PauseSystem::Settings)
        );
        assert_eq!(
            PauseSystem::from_name("controls"),
            Some(PauseSystem::Controls)
        );
        assert_eq!(PauseSystem::from_name("load"), Some(PauseSystem::Load));
        assert_eq!(PauseSystem::from_name("save"), Some(PauseSystem::Save));
        assert_eq!(PauseSystem::from_name("help"), Some(PauseSystem::Help));
        assert_eq!(PauseSystem::from_name("quit"), Some(PauseSystem::Quit));
        assert_eq!(PauseSystem::from_name("quests"), None);
        assert_eq!(PauseSystem::from_name("bogus"), None);
    }

    #[test]
    fn pause_seed_names_parse() {
        let quests = PauseSeed::from_name("quests").unwrap();
        assert_eq!(quests.tab, PauseTab::Quests);
        assert_eq!(quests.settings_options, None);
        let settings = PauseSeed::from_name("settings").unwrap();
        assert_eq!(settings.tab, PauseTab::System);
        assert_eq!(settings.system, PauseSystem::Settings);
        assert_eq!(settings.settings_options, Some(SettingsCategory::Gameplay));
        let display = PauseSeed::from_name("display").unwrap();
        assert_eq!(display.tab, PauseTab::System);
        assert_eq!(display.settings_options, Some(SettingsCategory::Display));
        let load = PauseSeed::from_name("load").unwrap();
        assert_eq!(load.tab, PauseTab::System);
        assert_eq!(load.system, PauseSystem::Load);
        assert_eq!(load.settings_options, None);
        assert!(PauseSeed::from_name("bogus").is_none());
    }

    #[test]
    fn pause_defaults_are_safe() {
        // Quit defaults to Cancel (index 1), never Quit.
        assert_eq!(PauseState::default().quit, 1);
        assert_eq!(PauseState::default().tab, PauseTab::Quests);
        assert_eq!(PauseState::default().system, PauseSystem::Settings);
        assert_eq!(
            PauseState::default().settings_category,
            SettingsCategory::Gameplay
        );
        assert_eq!(
            PauseState::default().settings_view,
            SettingsView::Categories
        );
    }
}
