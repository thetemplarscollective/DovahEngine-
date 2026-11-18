//! The character menus (UI milestone 2): Map, Magic, Items,
//! Skills — the game's menu, distinct from the pause screen
//! (Quests / Stats / System).
//!
//! Geometry comes from the game's own `.swf` files — see
//! `interface-extract/study/character_menu_study.md`. Data
//! comes from the real ESM: SPEL/MGEF/SHOU (`dovah_esm::spell`),
//! AVIF/RACE (`dovah_esm::skills`), XMRK markers
//! (`dovah_esm::map`), LAND heightfields for the map image.
//!
//! ## Deviations from the original (all documented in the study)
//!
//! * **Tab bar**: the real game cycles with Q/E and shows NO
//!   visible tabs. M2 adds a touch-friendly tab bar (Map |
//!   Magic | Items | Skills) across the top; content shifts down
//!   `CONTENT_Y` px to make room. Q/E cycle on desktop.
//! * **MENU stays pause**: MENU opens the pause screen (UI M1)
//!   — the phone's only pause access. The character menu opens
//!   from the MAP button (touch) + Tab (desktop, from gameplay
//!   only).
//! * **Items tab**: selecting Items closes this menu and opens
//!   the existing vanilla inventory (reused wholesale — NOT
//!   rebuilt, NO tab bar added to the verified layout). From the
//!   inventory, MAP re-opens the character menu (a handoff: the
//!   inventory closes a frame later); Tab still closes to game.
//! * **Magic list x**: the 820/1030 resting positions are a
//!   reconstruction — the `.swf` proves the lists are on the
//!   right and the card on the left, not the exact x.
//! * **Skills constellation**: the rotating 3D starfield is a
//!   later milestone. M2 shows the real skill data in the menu's
//!   chrome (top info, description card, bottom bar), grouped
//!   into the game's Warrior / Thief / Mage columns.
//! * **Perks**: honestly unwired — no perk tree, no AddPerk.
//!   The perk count shows 0 (the player has none at start).
//! * **Map image**: a height-shaded bake from the real LAND
//!   data, not the game's 3D map render. Markers are code-drawn
//!   diamonds (the `.swf`'s ~50 vector icons are a follow-up).
//! * **Discovery**: all markers show undiscovered-style — no
//!   visited state exists until the save system does.
//! * **Spell equip**: E... Enter/Space equips the selected spell
//!   as the single active spell (menu state only). Casting is a
//!   later milestone — the assignment has no gameplay effect
//!   yet. No two-hand assignment. (Q/E cycle tabs, so E cannot
//!   also equip — the bottom bar says so.)
//! * **Player marker indoors**: hidden — the engine does not
//!   track the player's Tamriel position while indoors (the
//!   local cell coords are not world coords). Outdoors it plots
//!   the real position.
//! * **Font**: Bevy's default font is ~30% wider than the game's
//!   condensed font (no extractable TTF — settled). Long names
//!   clip tails; geometry is `.swf`-faithful. Do NOT "fix" by
//!   shrinking fonts.

use crate::cell::{parse_xclc, PERSISTENT_CELL_FORM_ID, TAMRIEL_WRLD};
use crate::dialogue::GameStrings;
use crate::inventory::{fs, px, UiScale, STAGE_HEIGHT, STAGE_WIDTH};
use crate::land::{decode_land_heights, CELL_SPAN, LAND_SIZE, VERTEX_SPACING};
use crate::scene::{Player, TerrainHeights};
use crate::SharedPlugin;
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use dovah_esm::map::{marker_type_name, CITY_MARKER_CELLS, TAMRIEL_PERSISTENT_CELL};
use dovah_esm::skills::{parse_skill, race_skill_boosts, starting_skill_level, GUARDIAN_GROUPS};
use dovah_esm::spell::{
    mgef_skill, parse_spell, school_for_skill_index, spells_of, School, SpellKind,
};
use dovah_esm::{Group, GroupChild, GroupType, Record};
use std::collections::HashMap;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Geometry (stage coords, 1280x720 — see the study)
// ---------------------------------------------------------------------------

/// The tab bar's height + margin: tab content starts here.
const CONTENT_Y: f32 = 52.0;
/// Tab bar: y, height, per-tab width, font size.
const TAB_Y: f32 = 8.0;
const TAB_H: f32 = 36.0;
const TAB_W: f32 = 160.0;
const TAB_FONT: f32 = 24.0;

/// Magic: the spell card (magicmenu.swf ItemCardFadeHolder).
const MAGIC_CARD_X: f32 = 196.2;
const MAGIC_CARD_Y: f32 = 410.0;
const MAGIC_CARD_W: f32 = 380.0;
const MAGIC_CARD_H: f32 = 185.0;
/// Magic: the category + spell lists, translated right (see the
/// study's reconstruction — the `.swf` proves right-of-card,
/// not the exact x).
const MAGIC_CATEGORIES_X: f32 = 820.0;
const MAGIC_ITEMS_X: f32 = 1030.0;
const MAGIC_CATEGORY_W: f32 = 200.0;
const MAGIC_ITEM_W: f32 = 210.0;
const MAGIC_CATEGORY_PITCH: f32 = 40.0;
const MAGIC_ITEM_PITCH: f32 = 35.0;

/// Skills: top player info (statsmenu.swf TopPlayerInfo, abs
/// (640, 37)) + description card (abs (640, 523)).
const SKILLS_TOP_Y: f32 = 37.0;
const SKILLS_DESC_Y: f32 = 523.0;
const SKILLS_DESC_W: f32 = 700.0;
const SKILLS_DESC_H: f32 = 110.0;
const SKILLS_COL_W: f32 = 216.0;
const SKILLS_HEADER_Y: f32 = 140.0;
const SKILLS_ROWS_Y: f32 = 180.0;
const SKILLS_ROW_H: f32 = 40.0;

/// Map: the five bottom-bar buttons (map.swf Bottom sprite) +
/// the date text. X positions are the `.swf`'s.
const MAP_BUTTON_Y: f32 = 668.6;
const MAP_BUTTON_X: [f32; 5] = [85.0, 264.0, 443.0, 622.0, 801.0];
const MAP_BUTTON_W: f32 = 160.0;
const MAP_DATE_X: f32 = 927.0;
const MAP_DATE_Y: f32 = 670.6;
/// Map: the marker-info line above the bottom bar.
const MAP_INFO_Y: f32 = 615.0;
/// Map: the viewport's bounding box (the fitted image is
/// centered inside it, preserving the baked aspect).
const MAP_VIEW_X: f32 = 160.0;
const MAP_VIEW_Y: f32 = 60.0;
const MAP_VIEW_W: f32 = 960.0;
const MAP_VIEW_H: f32 = 545.0;

/// Bottom bar y (all menus).
const BOTTOM_BAR_Y: f32 = 645.0;

/// Tamriel's world extents in Skyrim units (LAND grid
/// x -57..61, y -43..50; cell span 4096). Proven by probe.
const MAP_MIN_X: f32 = -233_472.0;
const MAP_MAX_X: f32 = 253_952.0;
const MAP_MIN_Y: f32 = -176_128.0;
const MAP_MAX_Y: f32 = 208_896.0;
/// Map bake: one sample per second LAND vertex (17 per cell
/// edge) -> 1905 x 1505 px.
const MAP_BAKE_W: usize = 119 * 16 + 1;
const MAP_BAKE_H: usize = 94 * 16 + 1;
/// Tamriel's exterior cell count (the bake's grid: x -57..61,
/// y -43..50 — 119 x 94, proven by probe: every grid slot has
/// exactly one cell). The baker pre-sizes its maps to this so
/// no resize ever lands mid-bake (see `MapBaker::new`).
const TAMRIEL_CELL_COUNT: usize = 119 * 94;
/// Sample spacing in Skyrim units (2 vertices x 128).
const MAP_SAMPLE_STEP: f32 = 256.0;

/// Zoom levels for the map (1x = whole Tamriel).
const MAP_ZOOMS: [f32; 3] = [1.0, 2.0, 4.0];

/// Colors (the pause screen's palette).
const GOLD: Color = Color::srgb(0.85, 0.7, 0.35);
const TEXT_DIM: Color = Color::srgba(1.0, 1.0, 1.0, 0.7);
const TEXT_WHITE: Color = Color::WHITE;
const FADER_BG: Color = Color::srgba(0.0, 0.0, 0.0, 0.55);
const PANEL_BG: Color = Color::srgba(0.0, 0.0, 0.0, 0.6);
const PANEL_BORDER: Color = Color::srgba(0.6, 0.6, 0.6, 0.6);

/// The player record + the starting spells live here.
const PLAYER_FORM_ID: u32 = 0x0000_0007;

// ---------------------------------------------------------------------------
// Tabs + state
// ---------------------------------------------------------------------------

/// The character menu's tabs, in the game's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CharacterTab {
    #[default]
    Map,
    Magic,
    Items,
    Skills,
}

impl CharacterTab {
    pub fn from_name(name: &str) -> Option<CharacterTab> {
        match name.to_ascii_lowercase().as_str() {
            "map" => Some(CharacterTab::Map),
            "magic" => Some(CharacterTab::Magic),
            "items" => Some(CharacterTab::Items),
            "skills" => Some(CharacterTab::Skills),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CharacterTab::Map => "Map",
            CharacterTab::Magic => "Magic",
            CharacterTab::Items => "Items",
            CharacterTab::Skills => "Skills",
        }
    }

    /// The next tab (Q/E cycling skips Items — selecting Items
    /// hands off to the inventory, it is not a screen here).
    pub fn next(self) -> CharacterTab {
        match self {
            CharacterTab::Map => CharacterTab::Magic,
            CharacterTab::Magic => CharacterTab::Skills,
            CharacterTab::Skills => CharacterTab::Map,
            CharacterTab::Items => CharacterTab::Map,
        }
    }

    /// The previous tab (Q/E cycling skips Items).
    pub fn prev(self) -> CharacterTab {
        match self {
            CharacterTab::Map => CharacterTab::Skills,
            CharacterTab::Magic => CharacterTab::Map,
            CharacterTab::Skills => CharacterTab::Magic,
            CharacterTab::Items => CharacterTab::Map,
        }
    }
}

/// The Magic tab's categories: All, the five schools, Shouts,
/// Powers, and Active Effects (the game's fixed list, shown
/// always like the inventory's 10 categories).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagicCategory {
    All,
    Alteration,
    Conjuration,
    Destruction,
    Illusion,
    Restoration,
    Shouts,
    Powers,
    ActiveEffects,
}

impl MagicCategory {
    pub const ALL: [MagicCategory; 9] = [
        MagicCategory::All,
        MagicCategory::Alteration,
        MagicCategory::Conjuration,
        MagicCategory::Destruction,
        MagicCategory::Illusion,
        MagicCategory::Restoration,
        MagicCategory::Shouts,
        MagicCategory::Powers,
        MagicCategory::ActiveEffects,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MagicCategory::All => "All",
            MagicCategory::Alteration => "Alteration",
            MagicCategory::Conjuration => "Conjuration",
            MagicCategory::Destruction => "Destruction",
            MagicCategory::Illusion => "Illusion",
            MagicCategory::Restoration => "Restoration",
            MagicCategory::Shouts => "Shouts",
            MagicCategory::Powers => "Powers",
            MagicCategory::ActiveEffects => "Active Effects",
        }
    }

    /// The school this category filters to, if any.
    pub fn school(self) -> Option<School> {
        match self {
            MagicCategory::Alteration => Some(School::Alteration),
            MagicCategory::Conjuration => Some(School::Conjuration),
            MagicCategory::Destruction => Some(School::Destruction),
            MagicCategory::Illusion => Some(School::Illusion),
            MagicCategory::Restoration => Some(School::Restoration),
            _ => None,
        }
    }
}

/// Which Magic list has keyboard focus (Left/Right switch, like
/// the inventory).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MagicFocus {
    Categories,
    #[default]
    Items,
}

/// The character menu's open flag (gates movement/look/use
/// like `PauseOpen`).
#[derive(Resource, Default)]
pub struct CharacterMenuOpen(pub bool);

/// The character menu's state.
#[derive(Resource)]
pub struct CharacterState {
    pub tab: CharacterTab,
    pub magic_category: MagicCategory,
    pub magic_focus: MagicFocus,
    /// The selected spell's form ID.
    pub selected_spell: Option<u32>,
    /// The equipped spell's form ID (menu state only — casting
    /// is a later milestone).
    pub equipped_spell: Option<u32>,
    /// The selected skill's ActorValue index (6-23).
    pub selected_skill: u8,
    /// Map zoom level index into `MAP_ZOOMS`.
    pub map_zoom: usize,
    /// The map view's center in Skyrim (x, y). `None` = follow
    /// the player (outdoors) or the whole map (indoors).
    pub map_center: Option<(f32, f32)>,
    /// Local-map view (zoomed to the player's area) vs world.
    pub map_local: bool,
    /// The selected marker's ref form ID.
    pub selected_marker: Option<u32>,
}

impl Default for CharacterState {
    fn default() -> Self {
        CharacterState {
            tab: CharacterTab::Map,
            magic_category: MagicCategory::All,
            magic_focus: MagicFocus::Items,
            selected_spell: None,
            equipped_spell: None,
            selected_skill: 6, // One-Handed
            map_zoom: 0,
            map_center: None,
            map_local: false,
            selected_marker: None,
        }
    }
}

/// Rebuild the UI when set (the pause/inventory pattern).
#[derive(Resource, Default)]
pub struct CharacterDirty(pub bool);

/// The screenshot seed: open the character menu on this tab
/// (`--character-menu` / `--character-tab`, following
/// `--pause` / `--pause-tab`).
#[derive(Resource, Clone)]
pub struct CharacterSeed {
    pub tab: CharacterTab,
}

impl CharacterSeed {
    pub fn from_name(name: &str) -> Option<CharacterSeed> {
        CharacterTab::from_name(name).map(|tab| CharacterSeed { tab })
    }
}

/// Last frame's other-menus state — the Tab-that-closes-the-
/// inventory must not also open this menu in the same frame
/// (the pause screen's `PrevMenus` race, same fix).
#[derive(Resource, Default)]
struct PrevCharacterMenus {
    inventory: bool,
    dialogue: bool,
    pause: bool,
}

// ---------------------------------------------------------------------------
// Indexes (built once per app run in the OnEnter(InGame) chain)
// ---------------------------------------------------------------------------

/// One known spell, resolved for display.
#[derive(Debug, Clone)]
pub struct SpellEntry {
    pub form_id: u32,
    pub name: String,
    pub kind: SpellKind,
    /// The spell's school (first effect's MGEF skill). `None`
    /// for powers/abilities and non-school effects.
    pub school: Option<School>,
    pub cost: u32,
    /// Effect names (MGEF FULLs, in file order).
    pub effects: Vec<String>,
}

/// The player's known spells (player SPLO + race SPLO).
#[derive(Resource, Default)]
pub struct MagicIndex {
    pub spells: Vec<SpellEntry>,
    pub built: bool,
}

impl MagicIndex {
    pub fn build(plugin: &dovah_esm::Plugin, strings: &GameStrings) -> Self {
        let by_id: HashMap<u32, &dovah_esm::Record> = plugin
            .all_records()
            .into_iter()
            .map(|r| (r.form_id, r))
            .collect();
        // The player's spells: the player record's SPLO plus the
        // race record's SPLO (Resist Frost + Battle Cry for a
        // Nord). Dedup by form ID, keep file order.
        let mut spell_ids: Vec<u32> = Vec::new();
        if let Some(player) = by_id.get(&PLAYER_FORM_ID) {
            for id in spells_of(player) {
                if !spell_ids.contains(&id) {
                    spell_ids.push(id);
                }
            }
            if let Some(race_id) = player
                .field(b"RNAM")
                .and_then(|f| f.data.get(0..4))
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            {
                if let Some(race) = by_id.get(&race_id) {
                    for id in spells_of(race) {
                        if !spell_ids.contains(&id) {
                            spell_ids.push(id);
                        }
                    }
                }
            }
        }
        let mut spells = Vec::new();
        for id in spell_ids {
            let Some(record) = by_id.get(&id) else {
                continue;
            };
            let Some(spell) = parse_spell(record) else {
                continue;
            };
            let set = strings.set_for(spell.form_id);
            let name = spell
                .full_id
                .and_then(|i| set.general(i))
                .map(|s| s.to_string())
                .unwrap_or_else(|| spell.edid.clone().unwrap_or_else(|| "Unknown Spell".into()));
            // The school comes from the FIRST effect's MGEF skill
            // index (MGEF DATA @12).
            let school = spell
                .effects
                .first()
                .and_then(|mgef_id| by_id.get(mgef_id))
                .and_then(|mgef| mgef_skill(mgef))
                .and_then(school_for_skill_index);
            let effects = spell
                .effects
                .iter()
                .filter_map(|mgef_id| by_id.get(mgef_id))
                .filter_map(|mgef| {
                    mgef.field(b"FULL")
                        .and_then(|f| f.data.get(0..4))
                        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                        .and_then(|i| strings.set_for(mgef.form_id).general(i))
                        .map(|s| s.to_string())
                })
                .collect();
            spells.push(SpellEntry {
                form_id: spell.form_id,
                name,
                kind: spell.kind,
                school,
                cost: spell.cost,
                effects,
            });
        }
        MagicIndex {
            spells,
            built: true,
        }
    }
}

/// The spells visible under a category, in index order. All
/// shows the castable spells + powers (not abilities — the
/// game lists abilities under Active Effects only).
pub fn visible_spells(spells: &[SpellEntry], category: MagicCategory) -> Vec<&SpellEntry> {
    spells
        .iter()
        .filter(|s| match category {
            MagicCategory::All => {
                s.kind == SpellKind::Spell
                    || s.kind == SpellKind::Power
                    || s.kind == SpellKind::LesserPower
            }
            MagicCategory::Shouts => false, // no shouts known at start
            MagicCategory::Powers => s.kind == SpellKind::Power || s.kind == SpellKind::LesserPower,
            MagicCategory::ActiveEffects => s.kind == SpellKind::Ability,
            _ => {
                s.kind == SpellKind::Spell
                    && category.school().is_some_and(|sc| s.school == Some(sc))
            }
        })
        .collect()
}

/// One skill, resolved for display.
#[derive(Debug, Clone)]
pub struct SkillEntry {
    pub index: u8,
    pub name: String,
    pub desc: String,
    pub level: u8,
}

/// The 18 skills with the player's starting levels.
#[derive(Resource, Default)]
pub struct SkillIndex {
    pub skills: Vec<SkillEntry>,
    pub built: bool,
    /// The player's race name (RACE FULL), for the top info.
    pub race_name: String,
}

impl SkillIndex {
    pub fn build(plugin: &dovah_esm::Plugin, strings: &GameStrings) -> Self {
        let by_id: HashMap<u32, &dovah_esm::Record> = plugin
            .all_records()
            .into_iter()
            .map(|r| (r.form_id, r))
            .collect();
        // The player's race boosts (RACE DATA @0-13).
        let mut boosts: Vec<(u8, i8)> = Vec::new();
        let mut race_name = String::new();
        if let Some(player) = by_id.get(&PLAYER_FORM_ID) {
            if let Some(race_id) = player
                .field(b"RNAM")
                .and_then(|f| f.data.get(0..4))
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            {
                if let Some(race) = by_id.get(&race_id) {
                    boosts = race_skill_boosts(race);
                    race_name = race
                        .field(b"FULL")
                        .and_then(|f| f.data.get(0..4))
                        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                        .and_then(|i| strings.set_for(race_id).general(i))
                        .unwrap_or("")
                        .to_string();
                }
            }
        }
        let mut skills = Vec::new();
        for record in plugin.all_records() {
            let Some(skill) = parse_skill(record) else {
                continue;
            };
            let set = strings.set_for(skill.form_id);
            let name = skill
                .full_id
                .and_then(|i| set.general(i))
                .map(|s| s.to_string())
                .unwrap_or_else(|| skill.edid.clone().unwrap_or_else(|| "Unknown".into()));
            let desc = skill
                .desc_id
                .and_then(|i| set.longform(i))
                .unwrap_or("")
                .to_string();
            skills.push(SkillEntry {
                index: skill.index,
                name,
                desc,
                level: starting_skill_level(&boosts, skill.index),
            });
        }
        skills.sort_by_key(|s| s.index);
        SkillIndex {
            skills,
            built: true,
            race_name,
        }
    }

    pub fn get(&self, index: u8) -> Option<&SkillEntry> {
        self.skills.iter().find(|s| s.index == index)
    }
}

/// One world-map marker, resolved for display.
#[derive(Debug, Clone, PartialEq)]
pub struct MapMarker {
    pub ref_id: u32,
    pub name: String,
    pub type_name: &'static str,
    /// Skyrim units (x east, y north).
    pub x: f32,
    pub y: f32,
}

/// The world map: the baked height image + the 358 markers.
#[derive(Resource, Default)]
pub struct MapIndex {
    pub image: Option<Handle<Image>>,
    pub markers: Vec<MapMarker>,
    /// The marker diamond textures (white + gold): Bevy's UI
    /// layout owns `Transform`, so a rotated square node
    /// renders as a square — the diamond has to be an image.
    pub marker_image: Option<Handle<Image>>,
    pub player_image: Option<Handle<Image>>,
    pub built: bool,
}

impl MapIndex {
    /// One-shot build: drives a [`MapBaker`] to completion
    /// without a budget. Production never calls this (the
    /// Spawning MapBake phase drives the baker a slice at a
    /// time) — it exists as the chunked-vs-oneshot equivalence
    /// reference for tests: both paths run the SAME baker, so
    /// they cannot drift apart.
    pub fn build(
        plugin: &dovah_esm::Plugin,
        strings: &GameStrings,
        images: &mut Assets<Image>,
    ) -> Self {
        let mut baker = MapBaker::new();
        let deadline = Instant::now() + Duration::from_secs(3600);
        while !baker.step(plugin, deadline) {}
        baker.take_index(strings, images)
    }
}

/// A small diamond texture for map markers: a 24x24 image
/// whose filled pixels are the Manhattan diamond (|dx| + |dy|
/// <= 11.5 around the centre), transparent elsewhere. Pure —
/// unit-tested (corners transparent, centre + edge midpoints
/// filled).
fn diamond_image(color: Color) -> Image {
    const SIZE: usize = 24;
    let [r, g, b, a] = color.to_srgba().to_u8_array();
    let mut data = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = (x as f32 - 11.5).abs() + (y as f32 - 11.5).abs();
            if d <= 11.5 {
                let i = (y * SIZE + x) * 4;
                data[i] = r;
                data[i + 1] = g;
                data[i + 2] = b;
                data[i + 3] = a;
            }
        }
    }
    Image::new(
        Extent3d {
            width: SIZE as u32,
            height: SIZE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

// ---------------------------------------------------------------------------
// Map bake (pure helpers — unit-tested)
// ---------------------------------------------------------------------------

/// The (cell grid, vertex) a bake sample reads: pixel (px, py)
/// sits at world (MIN_X + px*256, MAX_Y - py*256). Row 0 is the
/// north edge (LAND row 0 is the SOUTH edge, so the vertex row
/// flips). Shared border vertices belong to either cell — the
/// heights match there.
pub fn sample_cell(px: usize, py: usize) -> ((i32, i32), (usize, usize)) {
    let wx = MAP_MIN_X + px as f32 * MAP_SAMPLE_STEP;
    let wy = MAP_MAX_Y - py as f32 * MAP_SAMPLE_STEP;
    let gx = (wx / CELL_SPAN).floor() as i32;
    let gy = (wy / CELL_SPAN).floor() as i32;
    // Clamp the border pixels into the edge cells (the north
    // edge sits exactly on the boundary above the top row).
    let gx = gx.clamp(-57, 61);
    let gy = gy.clamp(-43, 50);
    let vx = ((wx - gx as f32 * CELL_SPAN) / VERTEX_SPACING).round() as usize;
    let vy = ((wy - gy as f32 * CELL_SPAN) / VERTEX_SPACING).round() as usize;
    ((gx, gy), (vx.min(32), vy.min(32)))
}

/// A Skyrim (x, y) position as UV on the baked map: u=0 west,
/// u=1 east, v=0 north, v=1 south.
pub fn map_uv(x: f32, y: f32) -> (f32, f32) {
    (
        (x - MAP_MIN_X) / (MAP_MAX_X - MAP_MIN_X),
        (MAP_MAX_Y - y) / (MAP_MAX_Y - MAP_MIN_Y),
    )
}

/// The earth ramp: normalized height -> map color. A
/// stylization (the game paints its map); the heights are real.
pub fn height_shade(t: f32) -> [u8; 4] {
    const STOPS: [(f32, [u8; 3]); 5] = [
        (0.00, [38, 52, 36]),    // deep lowland green
        (0.35, [74, 88, 52]),    // olive
        (0.65, [122, 102, 66]),  // brown
        (0.85, [150, 138, 110]), // pale rock
        (1.00, [226, 224, 212]), // snow
    ];
    let t = t.clamp(0.0, 1.0);
    for w in STOPS.windows(2) {
        let (t0, c0) = w[0];
        let (t1, c1) = w[1];
        if t <= t1 {
            let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * f) as u8;
            return [
                lerp(c0[0], c1[0]),
                lerp(c0[1], c1[1]),
                lerp(c0[2], c1[2]),
                255,
            ];
        }
    }
    [226, 224, 212, 255]
}

/// One collected XMRK ref's owned field data. The marker
/// census runs during the Collect walk, when the plugin is
/// being traversed; names resolve at finish, when
/// `GameStrings` is at hand (the walk itself never touches
/// strings).
#[derive(Debug, Clone, PartialEq)]
struct MarkerRaw {
    ref_id: u32,
    /// FULL field string id, when the ref carries one.
    name_id: Option<u32>,
    tnam: u16,
    x: f32,
    y: f32,
}

/// The bake's stage. Collect discovers the plugin's Tamriel
/// cells + LAND VHGTs + XMRK refs; Decode turns VHGT bytes
/// into height grids; Sample reads the height grid into the
/// image's sample buffer (tracking the range); Shade paints
/// the buffer into RGBA (needs the global range, so it is a
/// separate stage).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BakeStage {
    Collect,
    Decode,
    Sample,
    Shade,
    Done,
}

/// A group's path from the plugin root as inline child
/// indexes — no heap allocation. The Collect walk clones one
/// path per subgroup it pushes, and the Tamriel WorldChildren
/// group has 11,186 subgroups: `Vec` clones there cost ~14 ms
/// of CPU in one step (map_baker_real_esm_step_times). ESM
/// group nesting is at most a few deep in Bethesda's data
/// (WorldChildren -> CellChildren), so 8 slots is far more
/// than enough; deeper paths panic loudly rather than
/// silently resolving the wrong group.
#[derive(Debug, Clone, Copy)]
struct GroupPath {
    indices: [usize; 8],
    len: usize,
}

impl GroupPath {
    fn root(i: usize) -> Self {
        GroupPath {
            indices: [i, 0, 0, 0, 0, 0, 0, 0],
            len: 1,
        }
    }

    fn child(&self, i: usize) -> Self {
        debug_assert!(self.len < 8, "group path deeper than 8");
        let mut indices = self.indices;
        indices[self.len] = i;
        GroupPath {
            indices,
            len: self.len + 1,
        }
    }

    fn as_slice(&self) -> &[usize] {
        &self.indices[..self.len]
    }
}

/// The world-map bake as an incremental state machine — the
/// map-bake ANR fix (UI M2 follow-up). The one-shot bake took
/// ~7 s on the main thread in OnEnter(InGame): TamrielCells
/// 75 ms + CellIndex 825 ms (built TWICE) + full LAND decode
/// 2.95 s + sampling 1.95 s + shading 0.39 s
/// (map_bake_probe). The baker does the same work a budget
/// slice at a time, driven by the Spawning MapBake phase — no
/// frame over budget, the loading screen alive with progress
/// throughout.
///
/// All intermediate state is OWNED. The plugin is borrowed
/// fresh each step (`&Plugin`): `TamrielCells`/`CellIndex`
/// borrow the plugin and cannot live in a Resource across
/// frames, so the Collect walk carries a DFS stack of
/// child-index PATHS (owned, re-resolved each step) and copies
/// out only what the later stages need — VHGT bytes per grid
/// cell + the marker refs' field data.
///
/// Two measured wins ride along (both output-identical,
/// proven by map_bake_probe + the chunked-vs-oneshot test):
/// the walk builds NO index (one pass collects LANDs and
/// markers together, replacing two 825 ms CellIndex builds),
/// and the decode is heights-only (the full decoder's
/// VCLR/VNML/VTXT work cost 2.3 s for data the bake threw
/// away).
pub(crate) struct MapBaker {
    stage: BakeStage,
    /// The Collect stack was seeded from the plugin's
    /// top-level groups.
    started: bool,
    /// DFS work stack for the Collect walk: (path of child
    /// indexes from the plugin root, in-Tamriel-subtree flag).
    /// XCLC grids are ambiguous across worldspaces, so CELL
    /// grids + LAND VHGTs are collected only inside the
    /// Tamriel WorldChildren subtree; the marker census is
    /// global (persistent-cell form IDs are unique).
    stack: Vec<(GroupPath, bool)>,
    /// The group being collected, mid-walk: (path,
    /// in-Tamriel flag, next child index). `None` between
    /// groups. The unit is a RECORD, not a group: the
    /// Tamriel WorldChildren group holds 11,186 CELLs (~75 ms
    /// in one gulp) and the Tamriel persistent group 15,196
    /// REFRs — group-sized units blew the 8 ms budget (the
    /// first version's max step was 93 ms).
    current: Option<(GroupPath, bool, usize)>,
    /// Groups visited in Collect (progress text only).
    groups_visited: usize,
    /// CELL form ID -> grid, Tamriel exterior cells only
    /// (the persistent cell is skipped — it shares grid
    /// (0,0) with the geographic cell; see
    /// `PERSISTENT_CELL_FORM_ID`).
    cell_grids: HashMap<u32, (i32, i32)>,
    /// CELL form ID -> owned VHGT bytes.
    land_vhgt: HashMap<u32, Vec<u8>>,
    /// Collected XMRK refs (owned field data).
    marker_raw: Vec<MarkerRaw>,
    /// Joined decode queue: (grid, VHGT bytes).
    decode_queue: Vec<((i32, i32), Vec<u8>)>,
    next_decode: usize,
    lands: HashMap<(i32, i32), [f32; LAND_SIZE * LAND_SIZE]>,
    heights: Vec<f32>,
    next_row: usize,
    min_h: f32,
    max_h: f32,
    data: Vec<u8>,
    next_shade_row: usize,
    /// Per-stage wall time (collect, decode, sample, shade),
    /// the slowest single step, and the slowest slice per
    /// stage, for the completion log. The per-stage max
    /// attributes a spike: Sample/Decode/Shade units are
    /// <=1.3 ms of work, so a spike there can only be the
    /// scheduler, while a spike in Collect points at the
    /// walk itself.
    stage_time: [Duration; 4],
    max_step: Duration,
    max_step_stage: [Duration; 4],
}

impl MapBaker {
    pub(crate) fn new() -> Self {
        MapBaker {
            stage: BakeStage::Collect,
            started: false,
            stack: Vec::new(),
            current: None,
            groups_visited: 0,
            // Pre-sized to the Tamriel grid: a HashMap resize
            // mid-bake copies every entry, and the lands map's
            // values are 4.3 KB each — the unreserved map
            // spiked to 90 ms of CPU at the 7,169-entry resize
            // (map_baker_real_esm_step_times). The reserve just
            // pre-allocates; a modded ESM with more cells still
            // works (the join reserves the exact count — a
            // no-op when this already covers it).
            cell_grids: HashMap::with_capacity(TAMRIEL_CELL_COUNT),
            land_vhgt: HashMap::with_capacity(TAMRIEL_CELL_COUNT),
            marker_raw: Vec::new(),
            decode_queue: Vec::new(),
            next_decode: 0,
            lands: HashMap::with_capacity(TAMRIEL_CELL_COUNT),
            heights: Vec::new(),
            next_row: 0,
            min_h: f32::MAX,
            max_h: f32::MIN,
            data: Vec::new(),
            next_shade_row: 0,
            stage_time: [Duration::ZERO; 4],
            max_step: Duration::ZERO,
            max_step_stage: [Duration::ZERO; 4],
        }
    }

    /// True before the first step (the Collect stack is not
    /// seeded yet) — the spawn system logs the bake's start
    /// on the first step.
    pub(crate) fn is_fresh(&self) -> bool {
        !self.started
    }

    /// Drive one budget slice of the bake. Returns true when
    /// the bake is complete (call `take_index` next). Each
    /// stage does at least one unit of work per call, so a
    /// zero budget still makes progress (one group / cell /
    /// row per frame — the SpawnBudget-zero test's pattern).
    /// One stage per step: a stage that completes hands over
    /// on the NEXT step, so a stage transition's fixed cost
    /// (the Collect→Decode join, the Sample buffer
    /// allocation) never chains into a full budget of the
    /// next stage's work in the same step.
    pub(crate) fn step(&mut self, plugin: &dovah_esm::Plugin, deadline: Instant) -> bool {
        let started = Instant::now();
        match self.stage {
            BakeStage::Collect => self.step_collect(plugin, deadline),
            BakeStage::Decode => self.step_decode(deadline),
            BakeStage::Sample => self.step_sample(deadline),
            BakeStage::Shade => self.step_shade(deadline),
            BakeStage::Done => {}
        }
        self.max_step = self.max_step.max(started.elapsed());
        self.stage == BakeStage::Done
    }

    /// Collect: one DFS slice over the plugin's groups, a
    /// RECORD at a time. Groups are opened in DFS order and
    /// their records drained until the deadline; a group cut
    /// off mid-way resumes from `current` on the next step.
    fn step_collect(&mut self, plugin: &dovah_esm::Plugin, deadline: Instant) {
        let started = Instant::now();
        if !self.started {
            // Seed: one frame per top-level group, reversed so
            // group 0 pops first. (Order is cosmetic — the
            // outputs are maps, and markers are sorted at
            // finish.)
            for i in (0..plugin.groups.len()).rev() {
                self.stack.push((GroupPath::root(i), false));
            }
            self.started = true;
        }
        loop {
            // Take the group in progress, or open the next one.
            let (path, in_tamriel, next) = match self.current.take() {
                Some(current) => current,
                None => {
                    let Some((path, in_tamriel)) = self.stack.pop() else {
                        break;
                    };
                    self.groups_visited += 1;
                    let Some(group) = resolve_group(plugin, path.as_slice()) else {
                        continue;
                    };
                    // The Tamriel WorldChildren group opens the
                    // subtree: CELL grids + LAND VHGTs are
                    // collected only inside it.
                    let in_tamriel = in_tamriel
                        || (group.group_type == GroupType::WorldChildren
                            && group.label_form_id() == Some(TAMRIEL_WRLD));
                    // Push subgroups (reversed: first child
                    // pops first).
                    for (i, child) in group.children.iter().enumerate().rev() {
                        if matches!(child, GroupChild::Group(_)) {
                            self.stack.push((path.child(i), in_tamriel));
                        }
                    }
                    (path, in_tamriel, 0)
                }
            };
            let Some(group) = resolve_group(plugin, path.as_slice()) else {
                continue;
            };
            match self.collect_records(group, in_tamriel, next, deadline) {
                Some(next) => {
                    // Deadline hit mid-group: resume here.
                    self.current = Some((path, in_tamriel, next));
                    break;
                }
                None => {
                    // Group exhausted: open the next one, if
                    // the budget allows.
                    if Instant::now() >= deadline {
                        break;
                    }
                }
            }
        }
        let elapsed = started.elapsed();
        self.stage_time[0] += elapsed;
        self.max_step_stage[0] = self.max_step_stage[0].max(elapsed);
        if self.stack.is_empty() && self.current.is_none() {
            // Join: grid -> VHGT bytes, for cells that have
            // both. (A LAND whose CELL never appeared — the
            // persistent cell's, or a decoy worldspace's —
            // never joins; a CELL without LAND decodes
            // nothing.) The join MOVES the VHGT bytes out of
            // the map (no 12 MB re-copy). ~1 ms — the one
            // unchunked unit in the bake, a single-unit
            // overrun like the chunked build's expensive
            // objects.
            let mut vhgt = std::mem::take(&mut self.land_vhgt);
            let grids = std::mem::take(&mut self.cell_grids);
            self.decode_queue = grids
                .into_iter()
                .filter_map(|(id, grid)| vhgt.remove(&id).map(|v| (grid, v)))
                .collect();
            // Reserve the lands map to the exact decode count:
            // its values are 4.3 KB each, so a mid-Decode
            // resize copies tens of MB (the 90 ms CPU spike —
            // see `MapBaker::new`). `new` already reserved the
            // Tamriel grid, so this is a no-op for the real
            // ESM — it is the backstop for a modded ESM with
            // more cells.
            self.lands.reserve(self.decode_queue.len());
            self.stage = BakeStage::Decode;
        }
    }

    /// Collect one group's records starting at child index
    /// `start`, until the deadline. Returns the next child
    /// index to resume at, or `None` when the group is
    /// exhausted. Group children that are subgroups are
    /// skipped here (they are on the DFS stack already).
    fn collect_records(
        &mut self,
        group: &Group,
        in_tamriel: bool,
        start: usize,
        deadline: Instant,
    ) -> Option<usize> {
        let is_cell_children = matches!(
            group.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        );
        let label = group.label_form_id();
        // The marker census: XMRK REFRs under the six
        // persistent cells (any of the three cell-children
        // group types — `CellIndex::scan` treats them the
        // same, and so does this walk).
        let is_marker_cell = is_cell_children
            && label
                .is_some_and(|l| l == TAMRIEL_PERSISTENT_CELL || CITY_MARKER_CELLS.contains(&l));
        if !in_tamriel && !is_marker_cell {
            return None;
        }
        let mut i = start;
        while i < group.children.len() {
            if let GroupChild::Record(record) = &group.children[i] {
                self.collect_record(record, in_tamriel, is_cell_children, is_marker_cell, label);
            }
            i += 1;
            if Instant::now() >= deadline {
                return Some(i);
            }
        }
        None
    }

    /// Collect one record: a Tamriel CELL's grid, a LAND's
    /// VHGT bytes, or an XMRK ref's field data. Direct byte
    /// compares, not `type_str()` (which allocates a String
    /// per record).
    fn collect_record(
        &mut self,
        record: &Record,
        in_tamriel: bool,
        is_cell_children: bool,
        is_marker_cell: bool,
        label: Option<u32>,
    ) {
        if in_tamriel && record.record_type == *b"CELL" {
            // Skip the persistent cell: it shares grid (0,0)
            // with the geographic cell (M9).
            if record.form_id == PERSISTENT_CELL_FORM_ID {
                return;
            }
            if let Some(f) = record.field(b"XCLC") {
                if let Some(grid) = parse_xclc(&f.data) {
                    // The bake's grid range — out-of-range
                    // cells are never sampled (`sample_cell`
                    // clamps), so they are never decoded.
                    if (-57..=61).contains(&grid.0) && (-43..=50).contains(&grid.1) {
                        self.cell_grids.insert(record.form_id, grid);
                    }
                }
            }
            return;
        }
        if !is_cell_children {
            return;
        }
        if in_tamriel && record.record_type == *b"LAND" {
            if let (Some(cell_id), Some(vhgt)) = (label, record.field(b"VHGT")) {
                // First LAND wins (a cell has one LAND;
                // defensive against duplicates).
                self.land_vhgt
                    .entry(cell_id)
                    .or_insert_with(|| vhgt.data.clone());
            }
            return;
        }
        if is_marker_cell && record.record_type == *b"REFR" {
            if record.field(b"XMRK").is_none() {
                return;
            }
            let Some(data) = record.field(b"DATA") else {
                return;
            };
            if data.data.len() < 12 {
                return;
            }
            let x = f32::from_le_bytes(data.data[0..4].try_into().unwrap());
            let y = f32::from_le_bytes(data.data[4..8].try_into().unwrap());
            let name_id = record
                .field(b"FULL")
                .and_then(|f| f.data.get(0..4))
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
            let tnam = record
                .field(b"TNAM")
                .and_then(|f| f.data.get(0..2))
                .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
                .unwrap_or(0);
            self.marker_raw.push(MarkerRaw {
                ref_id: record.form_id,
                name_id,
                tnam,
                x,
                y,
            });
        }
    }

    /// Decode: heights-only VHGT decode, one cell per unit.
    fn step_decode(&mut self, deadline: Instant) {
        let started = Instant::now();
        while let Some((grid, vhgt)) = self.decode_queue.get(self.next_decode) {
            if let Some(heights) = decode_land_heights(vhgt) {
                self.lands.insert(*grid, heights);
            }
            self.next_decode += 1;
            if Instant::now() >= deadline {
                break;
            }
        }
        let elapsed = started.elapsed();
        self.stage_time[1] += elapsed;
        self.max_step_stage[1] = self.max_step_stage[1].max(elapsed);
        if self.next_decode >= self.decode_queue.len() {
            self.decode_queue.clear();
            self.heights = vec![0.0f32; MAP_BAKE_W * MAP_BAKE_H];
            self.min_h = f32::MAX;
            self.max_h = f32::MIN;
            self.stage = BakeStage::Sample;
        }
    }

    /// Sample: the bake's height sampling, one row per unit —
    /// the same loop the one-shot bake ran (real `sample_cell`
    /// + the same heights map).
    fn step_sample(&mut self, deadline: Instant) {
        let started = Instant::now();
        loop {
            if self.next_row >= MAP_BAKE_H {
                break;
            }
            let py = self.next_row;
            for px in 0..MAP_BAKE_W {
                let ((gx, gy), (vx, vy)) = sample_cell(px, py);
                // LAND heights are row-major, row 0 = south edge.
                let h = self
                    .lands
                    .get(&(gx, gy))
                    .map(|h| h[vy * LAND_SIZE + vx])
                    .unwrap_or(0.0);
                self.heights[py * MAP_BAKE_W + px] = h;
                self.min_h = self.min_h.min(h);
                self.max_h = self.max_h.max(h);
            }
            self.next_row += 1;
            if Instant::now() >= deadline {
                break;
            }
        }
        let elapsed = started.elapsed();
        self.stage_time[2] += elapsed;
        self.max_step_stage[2] = self.max_step_stage[2].max(elapsed);
        if self.next_row >= MAP_BAKE_H {
            self.data = vec![0u8; MAP_BAKE_W * MAP_BAKE_H * 4];
            self.stage = BakeStage::Shade;
        }
    }

    /// Shade: the height ramp, one row per unit — the same
    /// loop the one-shot bake ran (real `height_shade`). A
    /// separate stage because shading needs the global range.
    fn step_shade(&mut self, deadline: Instant) {
        let started = Instant::now();
        let span = (self.max_h - self.min_h).max(1.0);
        loop {
            if self.next_shade_row >= MAP_BAKE_H {
                break;
            }
            let py = self.next_shade_row;
            for px in 0..MAP_BAKE_W {
                let i = py * MAP_BAKE_W + px;
                let rgba = height_shade((self.heights[i] - self.min_h) / span);
                self.data[i * 4..i * 4 + 4].copy_from_slice(&rgba);
            }
            self.next_shade_row += 1;
            if Instant::now() >= deadline {
                break;
            }
        }
        let elapsed = started.elapsed();
        self.stage_time[3] += elapsed;
        self.max_step_stage[3] = self.max_step_stage[3].max(elapsed);
        if self.next_shade_row >= MAP_BAKE_H {
            self.stage = BakeStage::Done;
        }
    }

    /// Bake progress 0.0–1.0, for the loading screen's
    /// progress bar. Stage weights are the measured costs on
    /// the desktop debug build (map_bake_probe: collect
    /// ~0.25 s, decode 0.62 s, sample 1.95 s, shade 0.39 s —
    /// total ~3.2 s): collect 0.08, decode 0.19, sample 0.61,
    /// shade 0.12. Collect has no known total (the DFS
    /// discovers groups), so it reports its floor until
    /// Decode starts.
    pub(crate) fn progress(&self) -> f32 {
        fn frac(done: usize, total: usize) -> f32 {
            if total == 0 {
                1.0
            } else {
                done as f32 / total as f32
            }
        }
        match self.stage {
            BakeStage::Collect => 0.0,
            BakeStage::Decode => 0.08 + 0.19 * frac(self.next_decode, self.decode_queue.len()),
            BakeStage::Sample => 0.27 + 0.61 * frac(self.next_row, MAP_BAKE_H),
            BakeStage::Shade => 0.88 + 0.12 * frac(self.next_shade_row, MAP_BAKE_H),
            BakeStage::Done => 1.0,
        }
    }

    /// The loading screen's line for the current stage.
    pub(crate) fn progress_text(&self) -> String {
        match self.stage {
            BakeStage::Collect => format!(
                "Drawing the map: collecting cells ({} groups)",
                self.groups_visited
            ),
            BakeStage::Decode => format!(
                "Drawing the map: decoding heights {}/{}",
                self.next_decode,
                self.decode_queue.len()
            ),
            BakeStage::Sample => format!(
                "Drawing the map: sampling row {}/{}",
                self.next_row, MAP_BAKE_H
            ),
            BakeStage::Shade => format!(
                "Drawing the map: shading row {}/{}",
                self.next_shade_row, MAP_BAKE_H
            ),
            BakeStage::Done => "Drawing the map: done".to_string(),
        }
    }

    /// One completion line for the startup log: per-stage wall
    /// time, the slowest single step (the ANR proof — it must
    /// stay far under 5 s), the slowest slice per stage (which
    /// stage the spike was in), and the counts.
    pub(crate) fn stats_line(&self) -> String {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        format!(
            "map bake done: collect {:.0} ms, decode {:.0} ms ({} cells), sample {:.0} ms, shade {:.0} ms, max step {:.1} ms [collect {:.1}, decode {:.1}, sample {:.1}, shade {:.1}], {} markers",
            ms(self.stage_time[0]),
            ms(self.stage_time[1]),
            self.lands.len(),
            ms(self.stage_time[2]),
            ms(self.stage_time[3]),
            ms(self.max_step),
            ms(self.max_step_stage[0]),
            ms(self.max_step_stage[1]),
            ms(self.max_step_stage[2]),
            ms(self.max_step_stage[3]),
            self.marker_raw.len()
        )
    }

    /// Take the finished bake as a `MapIndex`: the image, the
    /// markers (names resolved through `strings`), and the
    /// diamond textures. Call only after `step` returns true —
    /// the image data is incomplete before then.
    pub(crate) fn take_index(self, strings: &GameStrings, images: &mut Assets<Image>) -> MapIndex {
        debug_assert_eq!(self.stage, BakeStage::Done);
        let image = Image::new(
            Extent3d {
                width: MAP_BAKE_W as u32,
                height: MAP_BAKE_H as u32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            self.data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        let handle = images.add(image);
        let mut markers: Vec<MapMarker> = self
            .marker_raw
            .into_iter()
            .map(|m| MapMarker {
                ref_id: m.ref_id,
                name: m
                    .name_id
                    .and_then(|id| strings.set_for(m.ref_id).general(id))
                    .unwrap_or("")
                    .to_string(),
                type_name: marker_type_name(m.tnam),
                x: m.x,
                y: m.y,
            })
            .collect();
        markers.sort_by(|a, b| a.name.cmp(&b.name));
        let marker_image = images.add(diamond_image(Color::WHITE));
        let player_image = images.add(diamond_image(GOLD));
        MapIndex {
            image: Some(handle),
            markers,
            marker_image: Some(marker_image),
            player_image: Some(player_image),
            built: true,
        }
    }
}

/// Resolve a child-index path to its group: path[0] indexes
/// `plugin.groups`, each later index indexes the parent's
/// `children` (records and groups mixed — the path records
/// the position in `children`, not in `subgroups`).
fn resolve_group<'a>(plugin: &'a dovah_esm::Plugin, path: &[usize]) -> Option<&'a Group> {
    let mut group = plugin.groups.get(*path.first()?)?;
    for &i in &path[1..] {
        group = match group.children.get(i)? {
            GroupChild::Group(g) => g,
            GroupChild::Record(_) => return None,
        };
    }
    Some(group)
}

/// The map view transform: the displayed image size + the
/// offset of the image's top-left corner relative to the
/// viewport's, for a zoom + a center UV. The image is clamped
/// so it always covers the viewport (no void past the edges);
/// at zoom 1 the fit fills the viewport exactly.
pub fn map_view_transform(
    image_size: (f32, f32),
    viewport_size: (f32, f32),
    zoom: f32,
    center_uv: (f32, f32),
) -> ((f32, f32), (f32, f32)) {
    let display = (image_size.0 * zoom, image_size.1 * zoom);
    let mut off_x = viewport_size.0 / 2.0 - center_uv.0 * display.0;
    let mut off_y = viewport_size.1 / 2.0 - center_uv.1 * display.1;
    // Clamp: the image must cover the viewport.
    if display.0 >= viewport_size.0 {
        off_x = off_x.clamp(viewport_size.0 - display.0, 0.0);
    } else {
        off_x = (viewport_size.0 - display.0) / 2.0;
    }
    if display.1 >= viewport_size.1 {
        off_y = off_y.clamp(viewport_size.1 - display.1, 0.0);
    } else {
        off_y = (viewport_size.1 - display.1) / 2.0;
    }
    (display, (off_x, off_y))
}

/// A Bevy player position -> Skyrim map coords (x east, y
/// north). coords.rs: Bevy.x = Skyrim.x/70, Bevy.z =
/// -Skyrim.y/70.
pub fn bevy_to_skyrim_xy(pos: Vec3) -> (f32, f32) {
    (pos.x * 70.0, -pos.z * 70.0)
}

// ---------------------------------------------------------------------------
// Index builders (OnEnter(InGame) chain — run once, `built` guards)
// ---------------------------------------------------------------------------

/// Build the magic index (once per app run).
pub(crate) fn build_magic_index(
    plugin: Option<Res<SharedPlugin>>,
    strings: Option<Res<GameStrings>>,
    mut index: ResMut<MagicIndex>,
) {
    let (Some(plugin), Some(strings)) = (plugin, strings) else {
        return;
    };
    if index.built {
        return;
    };
    let started = std::time::Instant::now();
    *index = MagicIndex::build(&plugin.0, &strings);
    crate::startup_log::log(&format!(
        "magic index built: {} known spells ({} ms)",
        index.spells.len(),
        started.elapsed().as_millis()
    ));
}

/// Build the skill index (once per app run).
pub(crate) fn build_skill_index(
    plugin: Option<Res<SharedPlugin>>,
    strings: Option<Res<GameStrings>>,
    mut index: ResMut<SkillIndex>,
) {
    let (Some(plugin), Some(strings)) = (plugin, strings) else {
        return;
    };
    if index.built {
        return;
    };
    let started = std::time::Instant::now();
    *index = SkillIndex::build(&plugin.0, &strings);
    crate::startup_log::log(&format!(
        "skill index built: {} skills, race {} ({} ms)",
        index.skills.len(),
        index.race_name,
        started.elapsed().as_millis()
    ));
}

/// Apply the screenshot seed: open the character menu on the
/// requested tab. Also sets the menu scale from the capture
/// target (no window exists on the screenshot path) — the
/// pause seed's pattern.
#[allow(clippy::too_many_arguments)]
pub fn apply_character_seed(
    seed: Option<Res<CharacterSeed>>,
    mut state: ResMut<CharacterState>,
    mut open: ResMut<CharacterMenuOpen>,
    mut dirty: ResMut<CharacterDirty>,
    mut scale: ResMut<UiScale>,
    images: Res<Assets<Image>>,
    screenshot_target: Option<Res<crate::screenshot::ScreenshotTarget>>,
    magic: Res<MagicIndex>,
    mut inventory_request: ResMut<crate::inventory::InventoryRequest>,
) {
    let Some(seed) = seed else {
        return;
    };
    if let Some(target) = screenshot_target.as_deref() {
        if let Some(image) = images.get(&target.image) {
            scale.0 = image.height() as f32 / STAGE_HEIGHT;
        }
    }
    if seed.tab == CharacterTab::Items {
        // Items is the vanilla inventory, not a character-menu
        // screen — the seed hands off to it (the same request
        // the tab click sends) instead of opening this menu.
        inventory_request.0 = Some(crate::inventory::InventoryAction::Open);
        return;
    }
    state.tab = seed.tab;
    if seed.tab == CharacterTab::Magic {
        state.selected_spell = visible_spells(&magic.spells, state.magic_category)
            .first()
            .map(|s| s.form_id);
    }
    open.0 = true;
    dirty.0 = true;
}

// ---------------------------------------------------------------------------
// Open / close
// ---------------------------------------------------------------------------

/// Marker for the character menu's UI root.
#[derive(Component)]
pub struct CharacterUiRoot;

fn close_character_ui(
    commands: &mut Commands,
    ui_roots: &Query<Entity, With<CharacterUiRoot>>,
    open: &mut CharacterMenuOpen,
    dirty: &mut CharacterDirty,
) {
    for entity in ui_roots.iter() {
        commands.entity(entity).despawn();
    }
    open.0 = false;
    dirty.0 = false;
}

fn open_character_menu(
    open: &mut CharacterMenuOpen,
    dirty: &mut CharacterDirty,
    state: &mut CharacterState,
    magic: &MagicIndex,
) {
    open.0 = true;
    dirty.0 = true;
    // A spell is selected on open when the Magic tab shows —
    // the card is never empty when the category has spells.
    if state.selected_spell.is_none() {
        state.selected_spell = visible_spells(&magic.spells, state.magic_category)
            .first()
            .map(|s| s.form_id);
    }
    crate::startup_log::log("character menu opened");
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// The character menu's keys. When closed: Tab (desktop) or
/// MAP (touch) opens, guarded on the other menus (the pause
/// screen's `PrevMenus` race — a Tab that closes the inventory
/// must not also open this menu). MAP from the inventory hands
/// off (the inventory closes a frame later via the request).
/// When open: Tab/Esc/MAP closes; Q/E cycle tabs (skipping
/// Items); per-tab keys navigate.
#[allow(clippy::too_many_arguments)]
fn character_keys(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    touch: Option<Res<crate::touch::TouchInputState>>,
    inventory_open: Option<Res<crate::inventory::InventoryOpen>>,
    dialogue_open: Option<Res<crate::dialogue::DialogueOpen>>,
    pause_open: Option<Res<crate::pause::PauseOpen>>,
    mut prev: ResMut<PrevCharacterMenus>,
    mut open: ResMut<CharacterMenuOpen>,
    mut state: ResMut<CharacterState>,
    mut dirty: ResMut<CharacterDirty>,
    ui_roots: Query<Entity, With<CharacterUiRoot>>,
    magic: Res<MagicIndex>,
    mut inventory_request: ResMut<crate::inventory::InventoryRequest>,
    mut open_pause: ResMut<crate::pause::OpenPauseRequest>,
    player: Query<&Transform, With<Player>>,
    terrain: Option<Res<TerrainHeights>>,
) {
    let cur_inv = inventory_open.map(|o| o.0).unwrap_or(false);
    let cur_dlg = dialogue_open.map(|o| o.0).unwrap_or(false);
    let cur_pause = pause_open.map(|o| o.0).unwrap_or(false);
    let tab = keyboard.just_pressed(KeyCode::Tab);
    let esc = keyboard.just_pressed(KeyCode::Escape);
    let map_btn = touch.as_deref().map(|t| t.map_pressed).unwrap_or(false);
    let outdoors = terrain.is_some();

    if open.0 {
        if tab || esc || map_btn {
            close_character_ui(&mut commands, &ui_roots, &mut open, &mut dirty);
        } else if keyboard.just_pressed(KeyCode::KeyQ) {
            state.tab = state.tab.prev();
            dirty.0 = true;
        } else if keyboard.just_pressed(KeyCode::KeyE) {
            // E cycles tabs on every tab (skipping Items);
            // Magic equips with Enter/Space (see `magic_keys`)
            // so the two never collide.
            state.tab = state.tab.next();
            dirty.0 = true;
        } else {
            match state.tab {
                CharacterTab::Magic => {
                    magic_keys(&keyboard, &mut state, &mut dirty, &magic);
                }
                CharacterTab::Skills => {
                    skills_keys(&keyboard, &mut state, &mut dirty);
                }
                CharacterTab::Map => {
                    map_keys(
                        &keyboard,
                        &mut commands,
                        &ui_roots,
                        &mut state,
                        &mut dirty,
                        &mut open,
                        &mut open_pause,
                        &player,
                        outdoors,
                    );
                }
                CharacterTab::Items => {}
            }
        }
    } else if magic.built {
        // The indexes build on InGame entry; before that the
        // menu would open empty, so the open keys are ignored.
        if tab
            && !prev.inventory
            && !prev.dialogue
            && !prev.pause
            && !cur_inv
            && !cur_dlg
            && !cur_pause
        {
            open_character_menu(&mut open, &mut dirty, &mut state, &magic);
        } else if map_btn && !cur_dlg && !cur_pause {
            if cur_inv {
                // Handoff: the inventory closes a frame later
                // (the request system consumes it); this menu
                // opens now. One transient frame shows both.
                inventory_request.0 = Some(crate::inventory::InventoryAction::Close);
            }
            open_character_menu(&mut open, &mut dirty, &mut state, &magic);
        }
    }
    prev.inventory = cur_inv;
    prev.dialogue = cur_dlg;
    prev.pause = cur_pause;
}

/// Magic-tab keys: Left/Right switch focus (categories/items),
/// Up/Down move in the focused list, Enter/Space equips the
/// selected spell.
fn magic_keys(
    keyboard: &ButtonInput<KeyCode>,
    state: &mut CharacterState,
    dirty: &mut CharacterDirty,
    magic: &MagicIndex,
) {
    if keyboard.just_pressed(KeyCode::ArrowLeft) || keyboard.just_pressed(KeyCode::ArrowRight) {
        state.magic_focus = match state.magic_focus {
            MagicFocus::Categories => MagicFocus::Items,
            MagicFocus::Items => MagicFocus::Categories,
        };
        dirty.0 = true;
        return;
    }
    let up = keyboard.just_pressed(KeyCode::ArrowUp);
    let down = keyboard.just_pressed(KeyCode::ArrowDown);
    if up || down {
        let step: i32 = if down { 1 } else { -1 };
        match state.magic_focus {
            MagicFocus::Categories => {
                let pos = MagicCategory::ALL
                    .iter()
                    .position(|c| *c == state.magic_category)
                    .unwrap_or(0) as i32;
                let next = (pos + step).clamp(0, MagicCategory::ALL.len() as i32 - 1) as usize;
                if MagicCategory::ALL[next] != state.magic_category {
                    state.magic_category = MagicCategory::ALL[next];
                    // The selection follows the new category's
                    // first spell (the build falls back to it
                    // anyway when the old selection is not
                    // visible — this keeps the state in sync).
                    state.selected_spell = visible_spells(&magic.spells, state.magic_category)
                        .first()
                        .map(|s| s.form_id);
                    dirty.0 = true;
                }
            }
            MagicFocus::Items => {
                let visible = visible_spells(&magic.spells, state.magic_category);
                if visible.is_empty() {
                    return;
                }
                let pos = state
                    .selected_spell
                    .and_then(|id| visible.iter().position(|s| s.form_id == id))
                    .unwrap_or(0) as i32;
                let next = (pos + step).clamp(0, visible.len() as i32 - 1) as usize;
                let next_id = visible[next].form_id;
                if state.selected_spell != Some(next_id) {
                    state.selected_spell = Some(next_id);
                    dirty.0 = true;
                }
            }
        }
        return;
    }
    if keyboard.just_pressed(KeyCode::Enter) || keyboard.just_pressed(KeyCode::Space) {
        let visible = visible_spells(&magic.spells, state.magic_category);
        let displayed = state
            .selected_spell
            .filter(|id| visible.iter().any(|s| s.form_id == *id))
            .or_else(|| visible.first().map(|s| s.form_id));
        if let Some(id) = displayed {
            if state.equipped_spell != Some(id) {
                state.equipped_spell = Some(id);
                dirty.0 = true;
            }
        }
    }
}

/// Skills-tab keys: Up/Down move within a guardian column,
/// Left/Right move between columns.
fn skills_keys(
    keyboard: &ButtonInput<KeyCode>,
    state: &mut CharacterState,
    dirty: &mut CharacterDirty,
) {
    let up = keyboard.just_pressed(KeyCode::ArrowUp);
    let down = keyboard.just_pressed(KeyCode::ArrowDown);
    let left = keyboard.just_pressed(KeyCode::ArrowLeft);
    let right = keyboard.just_pressed(KeyCode::ArrowRight);
    if !(up || down || left || right) {
        return;
    }
    // Columns are [6..11], [12..17], [18..23] (see
    // GUARDIAN_GROUPS): col = (index-6)/6, row = (index-6)%6.
    let base = state.selected_skill.saturating_sub(6) as i32;
    let mut col = base / 6;
    let mut row = base % 6;
    if up {
        row = (row - 1).max(0);
    }
    if down {
        row = (row + 1).min(5);
    }
    if left {
        col = (col - 1).max(0);
    }
    if right {
        col = (col + 1).min(2);
    }
    let next = (6 + col * 6 + row) as u8;
    if next != state.selected_skill {
        state.selected_skill = next;
        dirty.0 = true;
    }
}

/// Map-tab keys: Left/Right zoom out/in, WASD pans, L toggles
/// the local view, J opens the journal (pause screen), P centers
/// on the player.
#[allow(clippy::too_many_arguments)]
fn map_keys(
    keyboard: &ButtonInput<KeyCode>,
    commands: &mut Commands,
    ui_roots: &Query<Entity, With<CharacterUiRoot>>,
    state: &mut CharacterState,
    dirty: &mut CharacterDirty,
    open: &mut CharacterMenuOpen,
    open_pause: &mut crate::pause::OpenPauseRequest,
    player: &Query<&Transform, With<Player>>,
    outdoors: bool,
) {
    if keyboard.just_pressed(KeyCode::ArrowLeft) {
        state.map_zoom = state.map_zoom.saturating_sub(1);
        dirty.0 = true;
        return;
    }
    if keyboard.just_pressed(KeyCode::ArrowRight) {
        state.map_zoom = (state.map_zoom + 1).min(MAP_ZOOMS.len() - 1);
        dirty.0 = true;
        return;
    }
    if keyboard.just_pressed(KeyCode::KeyL) {
        // Local view only works outdoors (it centers on the
        // player's Tamriel position).
        if outdoors {
            state.map_local = !state.map_local;
            state.map_zoom = if state.map_local { 2 } else { 0 };
            state.map_center = None;
            dirty.0 = true;
        }
        return;
    }
    if keyboard.just_pressed(KeyCode::KeyJ) {
        close_character_ui(commands, ui_roots, open, dirty);
        open_pause.0 = true;
        return;
    }
    if keyboard.just_pressed(KeyCode::KeyP) {
        state.map_center = None;
        dirty.0 = true;
        return;
    }
    // WASD pans (movement is gated while the menu is open, so
    // the keys are free). One press pans ~5% of the view.
    let mut dx = 0.0f32;
    let mut dy = 0.0f32;
    if keyboard.just_pressed(KeyCode::KeyW) {
        dy += 1.0;
    }
    if keyboard.just_pressed(KeyCode::KeyS) {
        dy -= 1.0;
    }
    if keyboard.just_pressed(KeyCode::KeyA) {
        dx -= 1.0;
    }
    if keyboard.just_pressed(KeyCode::KeyD) {
        dx += 1.0;
    }
    if dx != 0.0 || dy != 0.0 {
        let zoom = MAP_ZOOMS[state.map_zoom];
        let step = (MAP_MAX_X - MAP_MIN_X) * 0.05 / zoom;
        let (cx, cy) = state.map_center.unwrap_or_else(|| {
            player_center(player, outdoors)
                .unwrap_or(((MAP_MIN_X + MAP_MAX_X) / 2.0, (MAP_MIN_Y + MAP_MAX_Y) / 2.0))
        });
        let nx = (cx + dx * step).clamp(MAP_MIN_X, MAP_MAX_X);
        let ny = (cy + dy * step).clamp(MAP_MIN_Y, MAP_MAX_Y);
        state.map_center = Some((nx, ny));
        dirty.0 = true;
    }
}

/// The player's Tamriel position (outdoors only — indoors the
/// local cell coords are not world coords, so there is no
/// honest player position to plot).
fn player_center(player: &Query<&Transform, With<Player>>, outdoors: bool) -> Option<(f32, f32)> {
    if !outdoors {
        return None;
    }
    let pos = player.single().ok()?;
    Some(bevy_to_skyrim_xy(pos.translation))
}

/// Rebuild the UI when the state changed since the last build
/// (or the menu just opened). Runs every frame; cheap when
/// nothing is dirty.
fn refresh_character_ui(
    mut commands: Commands,
    ui_roots: Query<Entity, With<CharacterUiRoot>>,
    params: CharacterUiParams,
    open: Res<CharacterMenuOpen>,
    mut dirty: ResMut<CharacterDirty>,
) {
    if !open.0 || !dirty.0 {
        return;
    }
    for entity in ui_roots.iter() {
        commands.entity(entity).despawn();
    }
    build_character_ui(&mut commands, &params);
    dirty.0 = false;
}

// ---------------------------------------------------------------------------
// UI build
// ---------------------------------------------------------------------------

/// The UI-build parameters, bundled (Bevy's 16-parameter limit).
/// Two lifetimes: the query needs the state lifetime `'s`.
/// The item type is `&'static Transform` — the standard
/// SystemParam-derive pattern (the Query re-borrows at access
/// time; writing `&'w` there trips the derive's bounds).
#[derive(bevy::ecs::system::SystemParam)]
struct CharacterUiParams<'w, 's> {
    scale: Res<'w, UiScale>,
    state: Res<'w, CharacterState>,
    magic: Res<'w, MagicIndex>,
    skills: Res<'w, SkillIndex>,
    map: Res<'w, MapIndex>,
    player: Query<'w, 's, &'static Transform, With<Player>>,
    terrain: Option<Res<'w, TerrainHeights>>,
}

#[derive(Component)]
struct CharacterTabButton {
    tab: CharacterTab,
}

#[derive(Component)]
struct MagicCategoryButton {
    category: MagicCategory,
}

#[derive(Component)]
struct SpellRowButton {
    form_id: u32,
}

#[derive(Component)]
struct SkillRowButton {
    index: u8,
}

#[derive(Component)]
struct MarkerButton {
    ref_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MapButtonKind {
    LocalMap,
    Journal,
    ZoomOut,
    PlayerLoc,
    ZoomIn,
}

#[derive(Component)]
struct MapButton {
    kind: MapButtonKind,
}

/// Spawn one text node (absolute, stage coords) — the pause
/// screen's helper.
#[allow(clippy::too_many_arguments)]
fn text_node(
    parent: &mut bevy::ecs::prelude::ChildSpawnerCommands,
    text: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    size: f32,
    color: Color,
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

/// A text node whose overflow CLIPS instead of spilling: the
/// clip-parent pattern (the pause screen's fix for Bevy's
/// word-wrap) — a clipped parent node with the text as a
/// widthless child. Used for the skill description, whose
/// real `.DLSTRINGS` text is longer than the `.swf` card.
#[allow(clippy::too_many_arguments)]
fn clipped_text_node(
    parent: &mut bevy::ecs::prelude::ChildSpawnerCommands,
    text: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    size: f32,
    color: Color,
    scale: f32,
) {
    parent
        .spawn((Node {
            position_type: PositionType::Absolute,
            left: px(scale, x),
            top: px(scale, y),
            width: px(scale, w),
            height: px(scale, h),
            overflow: Overflow::clip(),
            ..Default::default()
        },))
        .with_children(|p| {
            p.spawn((
                Text::new(text),
                TextFont {
                    font_size: fs(scale, size),
                    ..Default::default()
                },
                TextColor(color),
            ));
        });
}

/// Spawn one centered text node (absolute, stage coords).
#[allow(clippy::too_many_arguments)]
fn centered_text_node(
    parent: &mut bevy::ecs::prelude::ChildSpawnerCommands,
    text: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    size: f32,
    color: Color,
    scale: f32,
) {
    parent
        .spawn((Node {
            position_type: PositionType::Absolute,
            left: px(scale, x),
            top: px(scale, y),
            width: px(scale, w),
            height: px(scale, h),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..Default::default()
        },))
        .with_children(|node| {
            node.spawn((
                Text::new(text),
                TextFont {
                    font_size: fs(scale, size),
                    ..Default::default()
                },
                TextColor(color),
            ));
        });
}

/// Build the whole character UI from the current state. Called
/// on open and on every state change (via `CharacterDirty`) — a
/// full rebuild, like the pause/inventory refresh.
fn build_character_ui(commands: &mut Commands, p: &CharacterUiParams) {
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
            CharacterUiRoot,
        ))
        .with_children(|root| {
            // The tab bar (a touch adaptation — the real game
            // shows no tabs): Map | Magic | Items | Skills,
            // centered.
            let tabs = [
                CharacterTab::Map,
                CharacterTab::Magic,
                CharacterTab::Items,
                CharacterTab::Skills,
            ];
            let bar_x = (STAGE_WIDTH - TAB_W * tabs.len() as f32) / 2.0;
            for (i, tab) in tabs.iter().enumerate() {
                let selected = state.tab == *tab;
                root.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(scale, bar_x + i as f32 * TAB_W),
                        top: px(scale, TAB_Y),
                        width: px(scale, TAB_W),
                        height: px(scale, TAB_H),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..Default::default()
                    },
                    CharacterTabButton { tab: *tab },
                ))
                .observe(on_character_tab_click)
                .with_children(|tab_node| {
                    tab_node.spawn((
                        Text::new(tab.label()),
                        TextFont {
                            font_size: fs(scale, TAB_FONT),
                            ..Default::default()
                        },
                        TextColor(if selected { GOLD } else { TEXT_DIM }),
                    ));
                });
            }

            match state.tab {
                CharacterTab::Map => build_map_ui(root, p),
                CharacterTab::Magic => build_magic_ui(root, p),
                CharacterTab::Skills => build_skills_ui(root, p),
                // Items never builds — selecting it hands off to
                // the inventory before any rebuild. Unreachable
                // in practice; the map is the safe fallback.
                CharacterTab::Items => build_map_ui(root, p),
            }
        });
}

/// The Magic tab: categories at x=820, spells at x=1030, the
/// spell card at (196, 410) — magicmenu.swf geometry (see the
/// study's reconstruction note on the list x).
fn build_magic_ui(root: &mut bevy::ecs::prelude::ChildSpawnerCommands, p: &CharacterUiParams) {
    let scale = p.scale.0;
    let state = &p.state;
    let visible = visible_spells(&p.magic.spells, state.magic_category);
    let displayed = state
        .selected_spell
        .filter(|id| visible.iter().any(|s| s.form_id == *id))
        .and_then(|id| visible.iter().find(|s| s.form_id == id))
        .or_else(|| visible.first());

    // Categories (40px pitch, .swf CategoryList).
    for (i, category) in MagicCategory::ALL.iter().enumerate() {
        let selected = state.magic_category == *category;
        let focused = state.magic_focus == MagicFocus::Categories;
        root.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(scale, MAGIC_CATEGORIES_X),
                top: px(scale, CONTENT_Y + i as f32 * MAGIC_CATEGORY_PITCH),
                width: px(scale, MAGIC_CATEGORY_W),
                height: px(scale, MAGIC_CATEGORY_PITCH),
                // Clip, don't wrap: Bevy's font is ~30% wider
                // than the game's condensed one, so "Active
                // Effects" wraps at the .swf's 150px — the field
                // is widened to 200px (the pause screen's
                // pattern) and any residual overflow clips its
                // tail.
                overflow: Overflow::clip(),
                ..Default::default()
            },
            MagicCategoryButton {
                category: *category,
            },
        ))
        .observe(on_magic_category_click)
        .with_children(|row| {
            row.spawn((
                Text::new(category.label()),
                TextFont {
                    font_size: fs(scale, if selected { 26.0 } else { 20.0 }),
                    ..Default::default()
                },
                TextColor(if selected {
                    GOLD
                } else if focused {
                    TEXT_WHITE
                } else {
                    TEXT_DIM
                }),
            ));
        });
    }

    // Spells (35px pitch, .swf ItemsList). The list is short at
    // start (the player knows 2-4 spells); a windowed scroll
    // with `centered_scroll_y` keeps long lists usable later.
    if visible.is_empty() {
        text_node(
            root,
            "(none)",
            MAGIC_ITEMS_X,
            CONTENT_Y,
            MAGIC_ITEM_W,
            MAGIC_ITEM_PITCH,
            20.0,
            TEXT_DIM,
            scale,
        );
    }
    let list_h = BOTTOM_BAR_Y - CONTENT_Y;
    let selected_pos = displayed
        .and_then(|d| visible.iter().position(|s| s.form_id == d.form_id))
        .unwrap_or(0);
    let content_h = visible.len() as f32 * MAGIC_ITEM_PITCH;
    let scroll = crate::inventory::centered_scroll_y(
        selected_pos as f32 * MAGIC_ITEM_PITCH,
        list_h / 2.0,
        content_h,
        list_h,
    );
    for (i, spell) in visible.iter().enumerate() {
        let y = CONTENT_Y + i as f32 * MAGIC_ITEM_PITCH - scroll;
        if y < CONTENT_Y - MAGIC_ITEM_PITCH || y > BOTTOM_BAR_Y {
            continue;
        }
        let selected = displayed
            .map(|d| d.form_id == spell.form_id)
            .unwrap_or(false);
        let equipped = state.equipped_spell == Some(spell.form_id);
        // The equipped marker (the .swf's EquipIcon slot, left
        // of the text).
        if equipped {
            text_node(
                root,
                ">",
                MAGIC_ITEMS_X - 18.0,
                y,
                18.0,
                MAGIC_ITEM_PITCH,
                20.0,
                GOLD,
                scale,
            );
        }
        root.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(scale, MAGIC_ITEMS_X),
                top: px(scale, y),
                width: px(scale, MAGIC_ITEM_W),
                height: px(scale, MAGIC_ITEM_PITCH),
                // Clip, don't wrap (same wide-font constraint
                // as the category rows — long spell names clip
                // tails).
                overflow: Overflow::clip(),
                ..Default::default()
            },
            SpellRowButton {
                form_id: spell.form_id,
            },
        ))
        .observe(on_spell_click)
        .with_children(|row| {
            row.spawn((
                Text::new(spell.name.clone()),
                TextFont {
                    font_size: fs(scale, 20.0),
                    ..Default::default()
                },
                TextColor(if selected { GOLD } else { TEXT_WHITE }),
            ));
        });
    }

    // The spell card (196, 410).
    root.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(scale, MAGIC_CARD_X),
            top: px(scale, MAGIC_CARD_Y),
            width: px(scale, MAGIC_CARD_W),
            height: px(scale, MAGIC_CARD_H),
            border: UiRect::all(Val::Px(1.0)),
            ..Default::default()
        },
        BackgroundColor(PANEL_BG),
        BorderColor::all(PANEL_BORDER),
    ));
    match displayed {
        Some(spell) => {
            text_node(
                root,
                &spell.name,
                MAGIC_CARD_X + 16.0,
                MAGIC_CARD_Y + 12.0,
                MAGIC_CARD_W - 32.0,
                32.0,
                26.0,
                TEXT_WHITE,
                scale,
            );
            let kind_line = match spell.kind {
                SpellKind::Spell => spell
                    .school
                    .map(|s| format!("{} - Spell", school_name(s)))
                    .unwrap_or_else(|| "Spell".to_string()),
                SpellKind::Power | SpellKind::LesserPower => "Power".to_string(),
                SpellKind::Ability => "Ability".to_string(),
                SpellKind::Other(_) => "Effect".to_string(),
            };
            text_node(
                root,
                &kind_line,
                MAGIC_CARD_X + 16.0,
                MAGIC_CARD_Y + 48.0,
                MAGIC_CARD_W - 32.0,
                26.0,
                20.0,
                TEXT_DIM,
                scale,
            );
            let mut y = MAGIC_CARD_Y + 78.0;
            if spell.kind == SpellKind::Spell {
                text_node(
                    root,
                    &format!("Magicka cost: {}", spell.cost),
                    MAGIC_CARD_X + 16.0,
                    y,
                    MAGIC_CARD_W - 32.0,
                    26.0,
                    20.0,
                    TEXT_WHITE,
                    scale,
                );
                y += 30.0;
            }
            for effect in &spell.effects {
                text_node(
                    root,
                    effect,
                    MAGIC_CARD_X + 16.0,
                    y,
                    MAGIC_CARD_W - 32.0,
                    26.0,
                    20.0,
                    TEXT_WHITE,
                    scale,
                );
                y += 28.0;
            }
            if state.equipped_spell == Some(spell.form_id) {
                text_node(
                    root,
                    "Equipped",
                    MAGIC_CARD_X + 16.0,
                    MAGIC_CARD_Y + MAGIC_CARD_H - 32.0,
                    MAGIC_CARD_W - 32.0,
                    26.0,
                    20.0,
                    GOLD,
                    scale,
                );
            }
        }
        None => {
            let empty = match state.magic_category {
                MagicCategory::Shouts => "No shouts known.",
                _ => "No spells known.",
            };
            text_node(
                root,
                empty,
                MAGIC_CARD_X + 16.0,
                MAGIC_CARD_Y + 12.0,
                MAGIC_CARD_W - 32.0,
                32.0,
                20.0,
                TEXT_DIM,
                scale,
            );
        }
    }

    // Bottom bar hints (touch: no key names — the tabs and
    // spells are tap targets, and MAP closes the menu).
    text_node(
        root,
        "Tap a tab to switch  Tap a spell twice to equip  MAP to close",
        20.0,
        BOTTOM_BAR_Y + 15.0,
        700.0,
        30.0,
        20.0,
        TEXT_DIM,
        scale,
    );
    text_node(
        root,
        &format!("Spells known: {}", p.magic.spells.len()),
        1000.0,
        BOTTOM_BAR_Y + 15.0,
        260.0,
        30.0,
        20.0,
        TEXT_DIM,
        scale,
    );
}

fn school_name(school: School) -> &'static str {
    match school {
        School::Alteration => "Alteration",
        School::Conjuration => "Conjuration",
        School::Destruction => "Destruction",
        School::Illusion => "Illusion",
        School::Restoration => "Restoration",
    }
}

/// The Skills tab: top player info, three guardian columns,
/// the description card — statsmenu.swf chrome (the rotating
/// constellation is a later milestone; see the study).
fn build_skills_ui(root: &mut bevy::ecs::prelude::ChildSpawnerCommands, p: &CharacterUiParams) {
    let scale = p.scale.0;
    let state = &p.state;

    // Top player info (640, 37 + content offset).
    let top_line = if p.skills.race_name.is_empty() {
        "LEVEL 1".to_string()
    } else {
        format!("LEVEL 1 - {}", p.skills.race_name)
    };
    centered_text_node(
        root,
        &top_line,
        440.0,
        CONTENT_Y + SKILLS_TOP_Y,
        400.0,
        34.0,
        28.0,
        TEXT_WHITE,
        scale,
    );
    centered_text_node(
        root,
        "Perks available: 0",
        440.0,
        CONTENT_Y + SKILLS_TOP_Y + 32.0,
        400.0,
        24.0,
        18.0,
        TEXT_DIM,
        scale,
    );

    // Three guardian columns (Warrior / Thief / Mage).
    let col_centers = [213.0, 640.0, 1067.0];
    for (col, (group, skills)) in GUARDIAN_GROUPS.iter().enumerate() {
        let x = col_centers[col] - SKILLS_COL_W / 2.0;
        centered_text_node(
            root,
            group,
            x,
            CONTENT_Y + SKILLS_HEADER_Y,
            SKILLS_COL_W,
            30.0,
            22.0,
            GOLD,
            scale,
        );
        for (row, skill_index) in skills.iter().enumerate() {
            let Some(skill) = p.skills.get(*skill_index) else {
                continue;
            };
            let selected = state.selected_skill == *skill_index;
            let y = CONTENT_Y + SKILLS_ROWS_Y + row as f32 * SKILLS_ROW_H;
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: px(scale, x),
                    top: px(scale, y),
                    width: px(scale, SKILLS_COL_W),
                    height: px(scale, SKILLS_ROW_H),
                    // Clip, don't wrap (same wide-font
                    // constraint).
                    overflow: Overflow::clip(),
                    ..Default::default()
                },
                SkillRowButton {
                    index: *skill_index,
                },
            ))
            .observe(on_skill_click)
            .with_children(|row_node| {
                row_node.spawn((
                    Text::new(format!("{}  {}", skill.name, skill.level)),
                    TextFont {
                        font_size: fs(scale, 20.0),
                        ..Default::default()
                    },
                    TextColor(if selected { GOLD } else { TEXT_WHITE }),
                ));
            });
        }
    }

    // The description card (640, 523).
    let card_x = (STAGE_WIDTH - SKILLS_DESC_W) / 2.0;
    root.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(scale, card_x),
            top: px(scale, SKILLS_DESC_Y),
            width: px(scale, SKILLS_DESC_W),
            height: px(scale, SKILLS_DESC_H),
            border: UiRect::all(Val::Px(1.0)),
            ..Default::default()
        },
        BackgroundColor(PANEL_BG),
        BorderColor::all(PANEL_BORDER),
    ));
    if let Some(skill) = p.skills.get(state.selected_skill) {
        text_node(
            root,
            &format!("{} - Level {}", skill.name, skill.level),
            card_x + 16.0,
            SKILLS_DESC_Y + 10.0,
            SKILLS_DESC_W - 32.0,
            28.0,
            22.0,
            GOLD,
            scale,
        );
        // Clipped: the real descriptions run 3-4 lines at this
        // width and would spill past the card's bottom border.
        clipped_text_node(
            root,
            &skill.desc,
            card_x + 16.0,
            SKILLS_DESC_Y + 40.0,
            SKILLS_DESC_W - 32.0,
            SKILLS_DESC_H - 48.0,
            18.0,
            TEXT_WHITE,
            scale,
        );
    }

    // Bottom bar hints (touch: no key names — the tabs and
    // skills are tap targets, and MAP closes the menu).
    text_node(
        root,
        "Tap a tab to switch  Tap a skill to select  MAP to close",
        20.0,
        BOTTOM_BAR_Y + 15.0,
        700.0,
        30.0,
        20.0,
        TEXT_DIM,
        scale,
    );
}

/// The Map tab: the baked height image in a clipped viewport,
/// markers + the player marker as children of the image (so
/// zoom/pan move them for free), the five `.swf` bottom-bar
/// buttons, the date, and the marker-info line.
fn build_map_ui(root: &mut bevy::ecs::prelude::ChildSpawnerCommands, p: &CharacterUiParams) {
    let scale = p.scale.0;
    let state = &p.state;
    let outdoors = p.terrain.is_some();
    let player_xy = player_center(&p.player, outdoors);

    // Fit the baked image into the viewport box, preserving
    // aspect (the world is 119:94, not 16:9 — the `.swf`'s
    // 800x450 viewport does not match the real map's shape).
    // The bake is always exactly MAP_BAKE_W x MAP_BAKE_H, so
    // the size is a constant — no asset lookup needed.
    let (img_w, img_h) = (MAP_BAKE_W as f32, MAP_BAKE_H as f32);
    let fit_scale = (MAP_VIEW_W / img_w).min(MAP_VIEW_H / img_h);
    let fit = (img_w * fit_scale, img_h * fit_scale);
    let view_x = MAP_VIEW_X + (MAP_VIEW_W - fit.0) / 2.0;
    let view_y = MAP_VIEW_Y + (MAP_VIEW_H - fit.1) / 2.0;

    // The view center: local view follows the player; an
    // explicit pan center wins; otherwise the player (outdoors)
    // or the map's middle (indoors).
    let center_uv = if state.map_local {
        player_xy.map(|(x, y)| map_uv(x, y)).unwrap_or((0.5, 0.5))
    } else if let Some((cx, cy)) = state.map_center {
        map_uv(cx, cy)
    } else {
        player_xy.map(|(x, y)| map_uv(x, y)).unwrap_or((0.5, 0.5))
    };
    let zoom = MAP_ZOOMS[state.map_zoom];
    let (display, offset) = map_view_transform(fit, fit, zoom, center_uv);

    // The viewport (clipped) + the image + the markers.
    root.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(scale, view_x),
            top: px(scale, view_y),
            width: px(scale, fit.0),
            height: px(scale, fit.1),
            overflow: Overflow::clip(),
            border: UiRect::all(Val::Px(1.0)),
            ..Default::default()
        },
        BackgroundColor(Color::BLACK),
        BorderColor::all(PANEL_BORDER),
    ))
    .with_children(|viewport| {
        let Some(handle) = p.map.image.clone() else {
            return;
        };
        viewport
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: px(scale, offset.0),
                    top: px(scale, offset.1),
                    width: px(scale, display.0),
                    height: px(scale, display.1),
                    ..Default::default()
                },
                ImageNode::new(handle),
            ))
            .with_children(|image| {
                // Markers: diamonds at their map UV. The
                // selected marker is gold and larger. The
                // diamond is an image (a rotated node renders
                // as a square — the UI layout owns Transform);
                // the BackgroundColor square is only a fallback
                // for an unbuilt index.
                for marker in &p.map.markers {
                    let (u, v) = map_uv(marker.x, marker.y);
                    let selected = state.selected_marker == Some(marker.ref_id);
                    let size = if selected { 12.0 } else { 8.0 };
                    let mut entity = image.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Percent(u * 100.0),
                            top: Val::Percent(v * 100.0),
                            width: px(scale, size),
                            height: px(scale, size),
                            margin: UiRect::all(px(scale, -size / 2.0)),
                            ..Default::default()
                        },
                        MarkerButton {
                            ref_id: marker.ref_id,
                        },
                    ));
                    let handle = if selected {
                        p.map.player_image.clone()
                    } else {
                        p.map.marker_image.clone()
                    };
                    match handle {
                        Some(handle) => {
                            entity.insert(ImageNode::new(handle));
                        }
                        None => {
                            entity.insert(BackgroundColor(if selected {
                                GOLD
                            } else {
                                Color::srgba(1.0, 1.0, 1.0, 0.85)
                            }));
                        }
                    }
                    entity.observe(on_marker_click);
                }
                // The player marker (outdoors only): a gold
                // diamond, larger than the location markers.
                if let Some((px_, py_)) = player_xy {
                    let (u, v) = map_uv(px_, py_);
                    let mut entity = image.spawn(Node {
                        position_type: PositionType::Absolute,
                        left: Val::Percent(u * 100.0),
                        top: Val::Percent(v * 100.0),
                        width: px(scale, 14.0),
                        height: px(scale, 14.0),
                        margin: UiRect::all(px(scale, -7.0)),
                        ..Default::default()
                    });
                    match p.map.player_image.clone() {
                        Some(handle) => {
                            entity.insert(ImageNode::new(handle));
                        }
                        None => {
                            entity.insert(BackgroundColor(GOLD));
                        }
                    }
                }
            });
    });

    // The marker-info line: the selected marker, else the
    // nearest marker to the player (outdoors), else the indoor
    // note.
    let info = if let Some(marker) = state
        .selected_marker
        .and_then(|id| p.map.markers.iter().find(|m| m.ref_id == id))
    {
        format!("{} - {}", marker.name, marker.type_name)
    } else if let Some((px_, py_)) = player_xy {
        p.map
            .markers
            .iter()
            .min_by(|a, b| {
                let da = (a.x - px_).powi(2) + (a.y - py_).powi(2);
                let db = (b.x - px_).powi(2) + (b.y - py_).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|m| format!("Nearest: {} - {}", m.name, m.type_name))
            .unwrap_or_else(|| "Tamriel".to_string())
    } else {
        "You are indoors - the map shows Tamriel".to_string()
    };
    centered_text_node(
        root, &info, MAP_VIEW_X, MAP_INFO_Y, MAP_VIEW_W, 26.0, 20.0, TEXT_WHITE, scale,
    );

    // The five bottom-bar buttons (map.swf Bottom sprite).
    let buttons = [
        (
            MapButtonKind::LocalMap,
            if state.map_local {
                "World Map"
            } else {
                "Local Map"
            },
        ),
        (MapButtonKind::Journal, "Journal"),
        // Short zoom labels: the .swf's fifth button sits at
        // x=801 and the date at x=927 — "Zoom In" at Bevy's
        // font width crowds the date (proven by the first map
        // render). The game's buttons are icons, not text.
        (MapButtonKind::ZoomOut, "Zoom -"),
        (MapButtonKind::PlayerLoc, "Player"),
        (MapButtonKind::ZoomIn, "Zoom +"),
    ];
    for (i, (kind, label)) in buttons.iter().enumerate() {
        root.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(scale, MAP_BUTTON_X[i]),
                top: px(scale, MAP_BUTTON_Y),
                width: px(scale, MAP_BUTTON_W),
                height: px(scale, 32.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            MapButton { kind: *kind },
        ))
        .observe(on_map_button_click)
        .with_children(|button| {
            button.spawn((
                Text::new(*label),
                TextFont {
                    font_size: fs(scale, 20.0),
                    ..Default::default()
                },
                TextColor(TEXT_WHITE),
            ));
        });
    }

    // The date (map.swf DateText).
    text_node(
        root,
        crate::pause::GAME_START_DATE,
        MAP_DATE_X,
        MAP_DATE_Y,
        300.0,
        28.0,
        20.0,
        TEXT_WHITE,
        scale,
    );
}

// ---------------------------------------------------------------------------
// Click observers
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn on_character_tab_click(
    event: On<PointerClick>,
    tabs: Query<&CharacterTabButton>,
    mut state: ResMut<CharacterState>,
    mut dirty: ResMut<CharacterDirty>,
    mut commands: Commands,
    ui_roots: Query<Entity, With<CharacterUiRoot>>,
    mut open: ResMut<CharacterMenuOpen>,
    mut inventory_request: ResMut<crate::inventory::InventoryRequest>,
) {
    let Ok(tab) = tabs.get(event.event_target()) else {
        return;
    };
    if tab.tab == CharacterTab::Items {
        // Items hands off to the vanilla inventory (reused
        // wholesale — this menu closes, the inventory opens).
        close_character_ui(&mut commands, &ui_roots, &mut open, &mut dirty);
        inventory_request.0 = Some(crate::inventory::InventoryAction::Open);
        return;
    }
    if state.tab != tab.tab {
        state.tab = tab.tab;
        dirty.0 = true;
    }
}

fn on_magic_category_click(
    event: On<PointerClick>,
    buttons: Query<&MagicCategoryButton>,
    mut state: ResMut<CharacterState>,
    mut dirty: ResMut<CharacterDirty>,
    magic: Res<MagicIndex>,
) {
    let Ok(button) = buttons.get(event.event_target()) else {
        return;
    };
    if state.magic_category != button.category {
        state.magic_category = button.category;
        state.selected_spell = visible_spells(&magic.spells, button.category)
            .first()
            .map(|s| s.form_id);
        dirty.0 = true;
    }
    state.magic_focus = MagicFocus::Categories;
}

fn on_spell_click(
    event: On<PointerClick>,
    rows: Query<&SpellRowButton>,
    mut state: ResMut<CharacterState>,
    mut dirty: ResMut<CharacterDirty>,
) {
    let Ok(row) = rows.get(event.event_target()) else {
        return;
    };
    // Tap to select; tap the selected spell again to equip it
    // (the touch path — desktop uses Enter/Space).
    if state.selected_spell == Some(row.form_id) {
        if state.equipped_spell != Some(row.form_id) {
            state.equipped_spell = Some(row.form_id);
            dirty.0 = true;
        }
    } else {
        state.selected_spell = Some(row.form_id);
        state.magic_focus = MagicFocus::Items;
        dirty.0 = true;
    }
}

fn on_skill_click(
    event: On<PointerClick>,
    rows: Query<&SkillRowButton>,
    mut state: ResMut<CharacterState>,
    mut dirty: ResMut<CharacterDirty>,
) {
    let Ok(row) = rows.get(event.event_target()) else {
        return;
    };
    if state.selected_skill != row.index {
        state.selected_skill = row.index;
        dirty.0 = true;
    }
}

fn on_marker_click(
    event: On<PointerClick>,
    markers: Query<&MarkerButton>,
    mut state: ResMut<CharacterState>,
    mut dirty: ResMut<CharacterDirty>,
) {
    let Ok(marker) = markers.get(event.event_target()) else {
        return;
    };
    if state.selected_marker != Some(marker.ref_id) {
        state.selected_marker = Some(marker.ref_id);
        dirty.0 = true;
    }
}

#[allow(clippy::too_many_arguments)]
fn on_map_button_click(
    event: On<PointerClick>,
    buttons: Query<&MapButton>,
    mut state: ResMut<CharacterState>,
    mut dirty: ResMut<CharacterDirty>,
    mut commands: Commands,
    ui_roots: Query<Entity, With<CharacterUiRoot>>,
    mut open: ResMut<CharacterMenuOpen>,
    mut open_pause: ResMut<crate::pause::OpenPauseRequest>,
    terrain: Option<Res<TerrainHeights>>,
) {
    let Ok(button) = buttons.get(event.event_target()) else {
        return;
    };
    match button.kind {
        MapButtonKind::LocalMap => {
            // Local view only works outdoors (it centers on the
            // player's Tamriel position).
            if terrain.is_some() {
                state.map_local = !state.map_local;
                state.map_zoom = if state.map_local { 2 } else { 0 };
                state.map_center = None;
                dirty.0 = true;
            }
        }
        MapButtonKind::Journal => {
            close_character_ui(&mut commands, &ui_roots, &mut open, &mut dirty);
            open_pause.0 = true;
        }
        MapButtonKind::ZoomOut => {
            state.map_zoom = state.map_zoom.saturating_sub(1);
            dirty.0 = true;
        }
        MapButtonKind::PlayerLoc => {
            state.map_center = None;
            dirty.0 = true;
        }
        MapButtonKind::ZoomIn => {
            state.map_zoom = (state.map_zoom + 1).min(MAP_ZOOMS.len() - 1);
            dirty.0 = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct CharacterPlugin;

/// Press log for the MAP button: the character menu opened or
/// closed this frame because of a MAP press. Runs after
/// `character_keys` in the Update tuple, so it sees the
/// transition in the frame it happens. (character_keys is at
/// Bevy's 16-parameter limit, so the outcome is observed here
/// instead of logged there — a keyboard Tab press finds no
/// pending MAP press and logs nothing.)
fn log_map_press_outcome(
    open: Res<CharacterMenuOpen>,
    mut press_log: Option<ResMut<crate::touch::PressLog>>,
) {
    if !open.is_changed() {
        return;
    }
    let Some(log) = press_log.as_deref_mut() else {
        return;
    };
    if let Some(count) = log.claim(crate::touch::PressButton::Map) {
        let outcome = if open.0 {
            "character menu opened"
        } else {
            "character menu closed"
        };
        crate::startup_log::log(&crate::touch::PressLog::format_line(
            crate::touch::PressButton::Map,
            count,
            Some(outcome),
        ));
    }
}

impl Plugin for CharacterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CharacterMenuOpen>()
            .init_resource::<CharacterState>()
            .init_resource::<CharacterDirty>()
            .init_resource::<MagicIndex>()
            .init_resource::<SkillIndex>()
            .init_resource::<MapIndex>()
            .init_resource::<PrevCharacterMenus>()
            .add_systems(
                Update,
                (character_keys, log_map_press_outcome, refresh_character_ui),
            );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn spell_entry(form_id: u32, kind: SpellKind, school: Option<School>) -> SpellEntry {
        SpellEntry {
            form_id,
            name: format!("Spell {form_id}"),
            kind,
            school,
            cost: 0,
            effects: Vec::new(),
        }
    }

    #[test]
    fn diamond_image_has_transparent_corners_and_a_filled_centre() {
        let img = diamond_image(Color::WHITE);
        assert_eq!(img.width(), 24);
        assert_eq!(img.height(), 24);
        let data = img.data.as_ref().unwrap();
        let alpha = |x: usize, y: usize| data[(y * 24 + x) * 4 + 3];
        // Corners transparent (outside the Manhattan diamond).
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(23, 0), 0);
        assert_eq!(alpha(0, 23), 0);
        assert_eq!(alpha(23, 23), 0);
        // Centre filled.
        assert_eq!(alpha(11, 11), 255);
        assert_eq!(alpha(12, 12), 255);
        // Tips: the 24px centre sits between pixels, so each
        // tip is 2px wide (rows/cols 1 and 22, not 0 and 23).
        assert_eq!(alpha(11, 1), 255);
        assert_eq!(alpha(12, 1), 255);
        assert_eq!(alpha(1, 11), 255);
        assert_eq!(alpha(22, 12), 255);
        // The white diamond is opaque white where filled.
        let rgb = |x: usize, y: usize| {
            (
                data[(y * 24 + x) * 4],
                data[(y * 24 + x) * 4 + 1],
                data[(y * 24 + x) * 4 + 2],
            )
        };
        assert_eq!(rgb(12, 12), (255, 255, 255));
    }

    #[test]
    fn character_tab_names_and_cycling_skip_items() {
        assert_eq!(CharacterTab::from_name("map"), Some(CharacterTab::Map));
        assert_eq!(CharacterTab::from_name("magic"), Some(CharacterTab::Magic));
        assert_eq!(CharacterTab::from_name("items"), Some(CharacterTab::Items));
        assert_eq!(
            CharacterTab::from_name("skills"),
            Some(CharacterTab::Skills)
        );
        assert_eq!(CharacterTab::from_name("nope"), None);
        assert_eq!(CharacterTab::from_name("MAGIC"), Some(CharacterTab::Magic));
        // Q/E cycling never lands on Items.
        assert_eq!(CharacterTab::Map.next(), CharacterTab::Magic);
        assert_eq!(CharacterTab::Magic.next(), CharacterTab::Skills);
        assert_eq!(CharacterTab::Skills.next(), CharacterTab::Map);
        assert_eq!(CharacterTab::Map.prev(), CharacterTab::Skills);
        assert_eq!(CharacterTab::Magic.prev(), CharacterTab::Map);
        assert_eq!(CharacterTab::Skills.prev(), CharacterTab::Magic);
        assert_eq!(CharacterTab::default(), CharacterTab::Map);
    }

    #[test]
    fn magic_categories_are_nine_with_five_schools() {
        assert_eq!(MagicCategory::ALL.len(), 9);
        assert_eq!(
            MagicCategory::Destruction.school(),
            Some(School::Destruction)
        );
        assert_eq!(MagicCategory::All.school(), None);
        assert_eq!(MagicCategory::Powers.school(), None);
        assert_eq!(MagicCategory::Shouts.label(), "Shouts");
        assert_eq!(MagicCategory::ActiveEffects.label(), "Active Effects");
    }

    #[test]
    fn visible_spells_filters_by_category() {
        // The starting set: Flames (Destruction spell), Healing
        // (Restoration spell), Battle Cry (Power), Resist Frost
        // (Ability).
        let spells = vec![
            spell_entry(1, SpellKind::Spell, Some(School::Destruction)),
            spell_entry(2, SpellKind::Spell, Some(School::Restoration)),
            spell_entry(3, SpellKind::Power, None),
            spell_entry(4, SpellKind::Ability, None),
        ];
        // All shows spells + powers, not abilities.
        let all = visible_spells(&spells, MagicCategory::All);
        assert_eq!(all.len(), 3);
        // Schools show only their spells.
        let dest = visible_spells(&spells, MagicCategory::Destruction);
        assert_eq!(dest.len(), 1);
        assert_eq!(dest[0].form_id, 1);
        let alt = visible_spells(&spells, MagicCategory::Alteration);
        assert!(alt.is_empty());
        // Powers / Active Effects / Shouts.
        assert_eq!(visible_spells(&spells, MagicCategory::Powers).len(), 1);
        assert_eq!(
            visible_spells(&spells, MagicCategory::ActiveEffects).len(),
            1
        );
        assert!(visible_spells(&spells, MagicCategory::Shouts).is_empty());
    }

    #[test]
    fn sample_cell_corners_and_center() {
        // North-west corner: cell (-57, 50), vertex (0, 32)
        // (north edge of the northmost cell).
        assert_eq!(sample_cell(0, 0), ((-57, 50), (0, 32)));
        // South-east corner: cell (61, -43), vertex (32, 0).
        assert_eq!(
            sample_cell(MAP_BAKE_W - 1, MAP_BAKE_H - 1),
            ((61, -43), (32, 0))
        );
        // The bake size follows from the grid (119x94 cells, 17
        // samples per edge).
        assert_eq!(MAP_BAKE_W, 1905);
        assert_eq!(MAP_BAKE_H, 1505);
    }

    #[test]
    fn map_uv_places_riverwood_south_central() {
        // Riverwood (21941, -44792): slightly east of center,
        // two-thirds down (south).
        let (u, v) = map_uv(21941.0, -44792.0);
        assert!((u - 0.524).abs() < 0.001, "u={u}");
        assert!((v - 0.659).abs() < 0.001, "v={v}");
        assert_eq!(map_uv(MAP_MIN_X, MAP_MAX_Y), (0.0, 0.0));
        assert_eq!(map_uv(MAP_MAX_X, MAP_MIN_Y), (1.0, 1.0));
    }

    #[test]
    fn height_shade_endpoints_and_order() {
        assert_eq!(height_shade(0.0), [38, 52, 36, 255]);
        assert_eq!(height_shade(1.0), [226, 224, 212, 255]);
        // Monotone-ish: the midpoint is between the ends on
        // every channel.
        let mid = height_shade(0.5);
        assert!(mid[0] > 38 && mid[0] < 226);
        assert!(mid[1] > 52 && mid[1] < 224);
    }

    #[test]
    fn map_view_transform_zoom_and_clamp() {
        // Zoom 1: the fit fills the viewport exactly.
        let ((w, h), (ox, oy)) =
            map_view_transform((690.0, 545.0), (690.0, 545.0), 1.0, (0.5, 0.5));
        assert_eq!((w, h), (690.0, 545.0));
        assert_eq!((ox, oy), (0.0, 0.0));
        // Zoom 2 centered: the image doubles, offset by half
        // the viewport.
        let ((w, h), (ox, oy)) =
            map_view_transform((690.0, 545.0), (690.0, 545.0), 2.0, (0.5, 0.5));
        assert_eq!((w, h), (1380.0, 1090.0));
        assert_eq!((ox, oy), (-345.0, -272.5));
        // Zoom 2 at the north-west corner: clamped so the image
        // still covers the viewport (offset 0,0).
        let (_, (ox, oy)) = map_view_transform((690.0, 545.0), (690.0, 545.0), 2.0, (0.0, 0.0));
        assert_eq!((ox, oy), (0.0, 0.0));
    }

    #[test]
    fn bevy_to_skyrim_xy_follows_coords() {
        // Bevy.x = Skyrim.x/70, Bevy.z = -Skyrim.y/70.
        assert_eq!(
            bevy_to_skyrim_xy(Vec3::new(100.0, 0.0, -200.0)),
            (7000.0, 14000.0)
        );
    }

    #[test]
    fn character_plugin_boots_a_bare_app() {
        // The plugin's systems take the inventory + pause
        // request resources and the shared menu scale — a
        // minimal app must init them (the real app's plugins
        // do: InventoryPlugin inits UiScale + the request,
        // PausePlugin inits the pause request).
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::input::InputPlugin)
            .init_resource::<crate::inventory::InventoryRequest>()
            .init_resource::<crate::inventory::UiScale>()
            .init_resource::<crate::pause::OpenPauseRequest>()
            .add_plugins(CharacterPlugin);
        app.update();
        assert!(!app.world().get_resource::<CharacterMenuOpen>().unwrap().0);
        assert!(app.world().get_resource::<MagicIndex>().is_some());
        assert!(app.world().get_resource::<MapIndex>().is_some());
    }

    /// A synthetic plugin for the baker tests: a Tamriel
    /// WorldChildren group with two geographic cells ((0,0)
    /// flat 100, (1,0) flat 200), the persistent cell at
    /// (0,0) with a decoy LAND (flat 999 — must be ignored),
    /// a decoy worldspace with a CELL at (0,0) + LAND flat
    /// 777 (must be ignored), and the Tamriel persistent
    /// cell's children with two XMRK REFRs (+ one REFR
    /// without XMRK, + one XMRK REFR under a non-marker
    /// cell — both must be skipped).
    fn baker_test_plugin() -> dovah_esm::Plugin {
        use dovah_esm::{Field, Group, GroupChild, GroupType, Plugin, Record};

        fn vhgt_bytes(first: f32) -> Vec<u8> {
            let mut bytes = Vec::with_capacity(1096);
            bytes.extend_from_slice(&first.to_le_bytes());
            bytes.extend(vec![0u8; LAND_SIZE * LAND_SIZE + 3]);
            bytes
        }

        fn cell(form_id: u32, gx: i32, gy: i32) -> Record {
            let mut data = Vec::with_capacity(12);
            data.extend_from_slice(&gx.to_le_bytes());
            data.extend_from_slice(&gy.to_le_bytes());
            data.extend_from_slice(&0u32.to_le_bytes());
            Record {
                record_type: *b"CELL",
                flags: 0,
                form_id,
                version: 44,
                fields: vec![Field {
                    field_type: *b"XCLC",
                    data,
                }],
            }
        }

        fn land(form_id: u32, first: f32) -> Record {
            Record {
                record_type: *b"LAND",
                flags: 0,
                form_id,
                version: 44,
                fields: vec![Field {
                    field_type: *b"VHGT",
                    data: vhgt_bytes(first),
                }],
            }
        }

        fn refr(
            form_id: u32,
            x: f32,
            y: f32,
            name_id: Option<u32>,
            tnam: u16,
            xmrk: bool,
        ) -> Record {
            let mut fields = Vec::new();
            let mut data = Vec::with_capacity(12);
            data.extend_from_slice(&x.to_le_bytes());
            data.extend_from_slice(&y.to_le_bytes());
            data.extend_from_slice(&0f32.to_le_bytes());
            fields.push(Field {
                field_type: *b"DATA",
                data,
            });
            if let Some(id) = name_id {
                fields.push(Field {
                    field_type: *b"FULL",
                    data: id.to_le_bytes().to_vec(),
                });
            }
            fields.push(Field {
                field_type: *b"TNAM",
                data: tnam.to_le_bytes().to_vec(),
            });
            if xmrk {
                fields.push(Field {
                    field_type: *b"XMRK",
                    data: Vec::new(),
                });
            }
            Record {
                record_type: *b"REFR",
                flags: 0,
                form_id,
                version: 44,
                fields,
            }
        }

        fn group(label: [u8; 4], group_type: GroupType, children: Vec<GroupChild>) -> Group {
            Group {
                label,
                group_type,
                children,
            }
        }

        let tamriel = group(
            TAMRIEL_WRLD.to_le_bytes(),
            GroupType::WorldChildren,
            vec![
                GroupChild::Record(cell(0x100, 0, 0)),
                GroupChild::Group(group(
                    0x100u32.to_le_bytes(),
                    GroupType::CellChildren,
                    vec![GroupChild::Record(land(0x101, 100.0))],
                )),
                GroupChild::Record(cell(0x200, 1, 0)),
                GroupChild::Group(group(
                    0x200u32.to_le_bytes(),
                    GroupType::CellChildren,
                    vec![
                        GroupChild::Record(land(0x201, 200.0)),
                        // XMRK under a non-marker cell: skipped.
                        GroupChild::Record(refr(0x202, 0.0, 0.0, None, 36, true)),
                    ],
                )),
                GroupChild::Record(cell(PERSISTENT_CELL_FORM_ID, 0, 0)),
                GroupChild::Group(group(
                    PERSISTENT_CELL_FORM_ID.to_le_bytes(),
                    GroupType::CellChildren,
                    vec![GroupChild::Record(land(0x203, 999.0))],
                )),
            ],
        );
        let decoy = group(
            0x1234u32.to_le_bytes(),
            GroupType::WorldChildren,
            vec![
                GroupChild::Record(cell(0x300, 0, 0)),
                GroupChild::Group(group(
                    0x300u32.to_le_bytes(),
                    GroupType::CellChildren,
                    vec![GroupChild::Record(land(0x301, 777.0))],
                )),
            ],
        );
        let persistent_children = group(
            TAMRIEL_PERSISTENT_CELL.to_le_bytes(),
            GroupType::CellPersistentChildren,
            vec![
                GroupChild::Record(refr(0x400, 1000.0, 2000.0, Some(42), 36, true)),
                GroupChild::Record(refr(0x401, 3000.0, 4000.0, None, 11, true)),
                // No XMRK: skipped.
                GroupChild::Record(refr(0x402, 5000.0, 6000.0, None, 36, false)),
            ],
        );
        Plugin {
            header: Record {
                record_type: *b"TES4",
                flags: 0,
                form_id: 0,
                version: 44,
                fields: Vec::new(),
            },
            groups: vec![tamriel, decoy, persistent_children],
        }
    }

    #[test]
    fn map_baker_chunked_equals_oneshot() {
        // The ANR fix's core promise: a bake split across
        // ~3,000 tiny steps (a past deadline every time) is
        // byte-identical to the one-shot bake — same image
        // bytes, same markers, same heights.
        let plugin = baker_test_plugin();
        let far_future = Instant::now() + Duration::from_secs(3600);
        let mut one = MapBaker::new();
        while !one.step(&plugin, far_future) {}
        let past = Instant::now() - Duration::from_secs(1);
        let mut chunked = MapBaker::new();
        let mut steps = 0;
        while !chunked.step(&plugin, past) {
            steps += 1;
            assert!(steps < 100_000, "the chunked bake did not terminate");
        }
        assert!(
            steps > 100,
            "the chunked bake finished in {steps} steps — it did not split"
        );
        assert_eq!(one.data, chunked.data);
        assert_eq!(one.marker_raw, chunked.marker_raw);
        assert_eq!(one.lands, chunked.lands);
        assert_eq!(one.min_h, chunked.min_h);
        assert_eq!(one.max_h, chunked.max_h);
    }

    #[test]
    fn map_baker_collects_only_tamriel_geographic_cells() {
        // The scoping rules: the persistent cell's LAND (flat
        // 999) and the decoy worldspace's LAND (flat 777) must
        // not shadow the geographic cell at (0,0) (flat 100);
        // only the two XMRK refs under the Tamriel persistent
        // cell are markers.
        let plugin = baker_test_plugin();
        let far_future = Instant::now() + Duration::from_secs(3600);
        let mut baker = MapBaker::new();
        while !baker.step(&plugin, far_future) {}
        assert_eq!(baker.lands.len(), 2);
        // ×8 height scale: flat 100 -> 800 everywhere.
        assert!(baker.lands[&(0, 0)].iter().all(|&h| h == 800.0));
        assert!(baker.lands[&(1, 0)].iter().all(|&h| h == 1600.0));
        assert_eq!(baker.marker_raw.len(), 2);
        let mut refs: Vec<u32> = baker.marker_raw.iter().map(|m| m.ref_id).collect();
        refs.sort_unstable();
        assert_eq!(refs, vec![0x400, 0x401]);
        let first = baker.marker_raw.iter().find(|m| m.ref_id == 0x400).unwrap();
        assert_eq!(first.name_id, Some(42));
        assert_eq!(first.tnam, 36);
        assert_eq!(first.x, 1000.0);
        assert_eq!(first.y, 2000.0);
    }

    #[test]
    fn map_baker_progress_is_monotonic_and_covers_all_stages() {
        let plugin = baker_test_plugin();
        let past = Instant::now() - Duration::from_secs(1);
        let mut baker = MapBaker::new();
        let mut last = 0.0f32;
        let mut stages = Vec::new();
        while !baker.step(&plugin, past) {
            let p = baker.progress();
            assert!(p >= last, "progress went backwards: {last} -> {p}");
            assert!(p <= 1.0);
            last = p;
            if !stages.contains(&baker.stage) {
                stages.push(baker.stage);
            }
        }
        assert_eq!(baker.progress(), 1.0);
        assert_eq!(baker.stage, BakeStage::Done);
        assert_eq!(
            stages,
            vec![
                BakeStage::Collect,
                BakeStage::Decode,
                BakeStage::Sample,
                BakeStage::Shade
            ]
        );
    }

    #[test]
    fn map_baker_take_index_builds_a_map_index() {
        let plugin = baker_test_plugin();
        let far_future = Instant::now() + Duration::from_secs(3600);
        let mut baker = MapBaker::new();
        while !baker.step(&plugin, far_future) {}
        let strings = GameStrings::default();
        let mut images = Assets::<Image>::default();
        let index = baker.take_index(&strings, &mut images);
        assert!(index.built);
        assert!(index.image.is_some());
        assert!(index.marker_image.is_some());
        assert!(index.player_image.is_some());
        assert_eq!(index.markers.len(), 2);
        let mut refs: Vec<u32> = index.markers.iter().map(|m| m.ref_id).collect();
        refs.sort_unstable();
        assert_eq!(refs, vec![0x400, 0x401]);
        // Empty strings: names fall back to "".
        assert!(index.markers.iter().all(|m| m.name.is_empty()));
    }

    /// The baker against the REAL Skyrim.esm (ignored by
    /// default: the parse alone is ~25 s). A measurement,
    /// not a logic test — the synthetic-plugin tests cover
    /// the logic. It measures the baker's per-step THREAD
    /// CPU time (not wall time): wall time is meaningless on
    /// this shared box — a concurrent link/render deschedules
    /// the process for 10s of ms at a stretch (every
    /// wall-clock max step measured so far, 88–310 ms, was
    /// descheduling, with all four stages spiking together)
    /// — but CPU time is the work bound, immune to the
    /// scheduler. The assertion is the ANR bound on CPU time
    /// (1 s): the bake's work per step must stay far under
    /// the 5 s ANR limit even before scheduling is counted.
    #[test]
    #[ignore]
    fn map_baker_real_esm_step_times() {
        /// This thread's CPU time (Linux): the work clock,
        /// immune to descheduling. `Instant` measures wall
        /// time, which is what the ANR limit is about — but
        /// on a shared box the wall time of a step is mostly
        /// time spent waiting for a CPU, not work done.
        fn thread_cpu_time() -> Duration {
            let mut ts = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // clock_gettime with a valid clock id + pointer
            // cannot fail on Linux; a nonzero return would be
            // a kernel bug, not a test failure mode.
            let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
            assert_eq!(rc, 0, "clock_gettime failed");
            Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
        }

        let esm =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/Skyrim.esm");
        let bytes = std::fs::read(&esm).expect("test-data/Skyrim.esm");
        let plugin = dovah_esm::Plugin::from_bytes(&bytes).expect("parse Skyrim.esm");
        let budget = Duration::from_millis(8);
        let mut baker = MapBaker::new();
        let mut steps = 0;
        let mut max_cpu_step = Duration::ZERO;
        let mut total_cpu = Duration::ZERO;
        // The five slowest steps, with the baker's state at
        // each — to attribute a spike to its stage + unit.
        let mut slowest: Vec<(Duration, &'static str, usize, usize, usize)> = Vec::new();
        loop {
            let cpu0 = thread_cpu_time();
            let done = baker.step(&plugin, Instant::now() + budget);
            let cpu = thread_cpu_time() - cpu0;
            max_cpu_step = max_cpu_step.max(cpu);
            total_cpu += cpu;
            steps += 1;
            let (stage, a, b, c) = match baker.stage {
                BakeStage::Collect => ("collect", baker.groups_visited, baker.stack.len(), 0),
                BakeStage::Decode => ("decode", baker.next_decode, baker.lands.len(), 0),
                BakeStage::Sample => ("sample", baker.next_row, 0, 0),
                BakeStage::Shade => ("shade", baker.next_shade_row, 0, 0),
                BakeStage::Done => ("done", 0, 0, 0),
            };
            slowest.push((cpu, stage, a, b, c));
            slowest.sort_by(|x, y| y.0.cmp(&x.0));
            slowest.truncate(5);
            if done {
                break;
            }
        }
        println!(
            "real-ESM bake: {steps} steps, wall {}, CPU total {:.0} ms, max CPU step {:.1} ms",
            baker.stats_line(),
            total_cpu.as_secs_f64() * 1000.0,
            max_cpu_step.as_secs_f64() * 1000.0
        );
        for (cpu, stage, a, b, _c) in &slowest {
            println!(
                "  slowest: {stage} {:.1} ms (a={a}, b={b})",
                cpu.as_secs_f64() * 1000.0
            );
        }
        assert_eq!(baker.lands.len(), 11_186);
        assert_eq!(baker.marker_raw.len(), 358);
        assert!(
            max_cpu_step < Duration::from_millis(1000),
            "max CPU step {max_cpu_step:?} — the bake's work blocked the main thread"
        );
    }
}
