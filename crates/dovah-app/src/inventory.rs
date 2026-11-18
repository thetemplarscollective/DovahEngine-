//! Inventory (M17, true vanilla layout): the player's bag —
//! items, gold, equipment — and the inventory screen.
//!
//! The screen IS Skyrim's inventory (Dylan: "I want the original
//! system"), built from the game's own Interface.bsa menus
//! (interface-extract/ — the .swf dumps are the authority, and
//! every position below cites them): the category list + item
//! list side by side on the LEFT (categories x=20, items x=230
//! at the 1280x720 stage), the SELECTED ITEM's 3D model on the
//! right (rotatable on mouse drag — item_model.rs), the item
//! card below it (holder x=532, y=434.3), the bottom bar at
//! y=645 (Damage / Armor / Carry Weight / Gold). NO character
//! doll — vanilla has none (the doll is a mod thing); the doll
//! renderer (player_model.rs) stays intact for the
//! character/level-up screens, it is just not shown here. Ten
//! categories INCLUDING Scrolls (Favorites is the Q menu, not a
//! category). The tabs/doll redesign core that shipped in
//! v0.4.4 is gone.
//!
//! Controls (vanilla PC): Left/Right switch focus between the
//! category and item lists, Up/Down navigate the focused list
//! (both lists wrap), E equips/uses (or focuses the items when
//! the categories are focused), R drops, Tab closes, B toggles,
//! Esc closes. Tapping a category row selects it and focuses
//! the items; tapping a row selects it, tapping the selected
//! row equips/uses it; the bottom-bar buttons are tappable
//! (the touch path for E / R / close). The lists are CENTERED:
//! the selected row sits at a fixed position and the list
//! scrolls around it, as in the game's CenteredScrollingList.
//!
//! The stage is 1280x720 (the .swf stage); `UiScale` (window
//! height / 720) scales every position, size, and font at build
//! time, so the menu keeps its proportions at any resolution.
//!
//! Font: fonts_en.swf holds glyph outlines only — NO
//! extractable TTF (settled 2026-11-15) — so the screen keeps
//! the current font. The key icons (E / R / Tab) ARE the game's
//! own textures, loaded at runtime from Interface.bsa
//! (`interface/exported/*.png.dds`) — never bundled, the same
//! rule as the HUD art. The equipped arrow, dividers, and panel
//! backgrounds are vector shapes in the .swf, so they are drawn
//! with UI primitives (">" for the arrow — the default font has
//! no triangle glyph).
//!
//! Data comes from the game's own records, parsed with the layouts
//! proven by `item_probe` / `item_probe2` against the real
//! Skyrim.esm (no guessing):
//! - Display names are the REAL names from `.strings` (M18): the
//!   record's `FULL` field is a string ID into the plugin's
//!   `.STRINGS` file ("FoodSweetroll" -> "Sweetroll"). When the
//!   strings are unavailable or the ID does not resolve, the
//!   name falls back to the base EDID prettified ("IronSword" ->
//!   "Iron Sword") — the M17 stand-in.
//! - Value/weight: MISC/WEAP/ARMO `DATA` = value i32 + weight f32;
//!   WEAP damage = `DATA` u16 at offset 8 (IronSword 25/9/7,
//!   IronDagger 10/2/4 — the real game's numbers).
//! - ARMO rating = `DNAM` u32 / 100 (Iron Cuirass 2500 -> 25);
//!   slots = `BOD2` first u32 (body 0x04, hands 0x08, feet 0x80,
//!   shield 0x200, head bits 0x01|0x02|0x1000).
//! - ALCH: value = `ENIT` first u32, weight = `DATA` f32, food =
//!   `ENIT` flags bit 0 (foods have flags 3, potions 0).
//! - BOOK: value = `DATA` i32 at offset 8, weight = f32 at 12.
//! - Gold (base 0x0000000f) is a counter, not a stack — as in the
//!   game.
//!
//! The `ItemCatalog` is built once per app run from the parsed
//! plugin (one pass over all records): every item definition,
//! every leveled list, every container's contents, the player's
//! starting outfit (NPC_ 0x00000007's DOFT), and the player's
//! body/head models. Everything downstream reads owned data from
//! the catalog — no record chasing at pickup time.
//!
//! Leveled lists (LVLI) resolve DETERMINISTICALLY in M17: the
//! entry picked is a hash of (list, container), so the same
//! container always gives the same loot (good for testing) while
//! different containers vary. The real game rolls randomly and
//! honors "chance none" / "use all" — those are ignored here and
//! noted as the M17 approximation.

use crate::npc;
use bevy::app::{App, Plugin, Startup, Update};
use bevy::asset::{Assets, Handle};
use bevy::color::Color;
use bevy::ecs::prelude::*;
use bevy::image::Image;
use bevy::input::keyboard::KeyCode;
use bevy::input::ButtonInput;
use bevy::math::{Quat, Vec2, Vec3};
use bevy::mesh::Mesh;
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::picking::events::PointerClick;
use bevy::render::mesh::{Mesh3d, VertexAttributeValues};
use bevy::text::{FontSize, Justify, TextColor, TextFont, TextLayout};
use bevy::transform::components::Transform;
use bevy::ui::widget::{ImageNode, Text};
use bevy::ui::{
    AlignItems, BackgroundColor, BorderColor, FlexDirection, JustifyContent, Node, Overflow,
    PositionType, ScrollPosition, Val,
};
use bevy::window::{PrimaryWindow, Window};
use dovah_esm::{Plugin as EsmPlugin, Record};
use std::collections::HashMap;

/// Gold's base form ID (MISC `Gold001`): picked-up gold adds to
/// the gold counter, never to a stack.
pub const GOLD_FORM_ID: u32 = 0x0000_000f;
/// The player character's NPC_ form ID.
pub const PLAYER_FORM_ID: u32 = 0x0000_0007;
/// Base carry capacity, as in the game. Shown in the UI; not
/// enforced in M17 (no over-encumbered state yet).
pub const CARRY_CAPACITY: f32 = 300.0;

/// What kind of item a base record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Weapon,
    Armor,
    Food,
    Potion,
    Book,
    Ingredient,
    Ammo,
    Misc,
    Key,
    SoulGem,
    Note,
    Scroll,
}

impl ItemKind {
    /// The card's type line (the .swf's SkillTextInstance slot,
    /// at the card's top): the item's kind in small caps. The
    /// game shows the skill for weapons ("One-Handed") and the
    /// armor class for armor ("Heavy Armor") — that needs
    /// weapon-type / keyword parsing the catalog does not do
    /// yet, so the kind stands in (a documented approximation,
    /// not the game's text).
    pub fn type_label(self) -> &'static str {
        match self {
            ItemKind::Weapon => "WEAPON",
            ItemKind::Armor => "APPAREL",
            ItemKind::Food => "FOOD",
            ItemKind::Potion => "POTION",
            ItemKind::Book => "BOOK",
            ItemKind::Ingredient => "INGREDIENT",
            ItemKind::Ammo => "AMMO",
            ItemKind::Misc => "MISC",
            ItemKind::Key => "KEY",
            ItemKind::SoulGem => "SOUL GEM",
            ItemKind::Note => "NOTE",
            ItemKind::Scroll => "SCROLL",
        }
    }
}

/// Where an armor piece is worn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EquipSlot {
    Head,
    Body,
    Hands,
    Feet,
    Shield,
}

/// One item definition, parsed from its base record and owned by
/// the catalog.
#[derive(Debug, Clone)]
pub struct ItemDef {
    /// The def's own identity (the catalog key is the same value,
    /// but a def passed around on its own still needs to know
    /// which item it is — the equip/use logs and any future
    /// debugger read it from here, not the key).
    #[allow(dead_code)]
    pub base_form_id: u32,
    pub name: String,
    pub kind: ItemKind,
    pub value: i32,
    pub weight: f32,
    /// Weapon damage (WEAP `DATA` u16 at offset 8; AMMO too when
    /// the record carries it).
    pub damage: u16,
    /// Weapon swing speed (WEAP `DNAM` f32 at offset 4; sword
    /// 1.0, dagger 1.3, greatsword 0.7). The swing cooldown is
    /// 1 / speed seconds. 1.0 for non-weapons.
    pub speed: f32,
    /// Weapon reach stat (WEAP `DNAM` f32 at offset 8; sword
    /// 1.0, dagger 0.7, greatsword 1.3). Multiplies the 1.5 m
    /// base melee reach. 1.0 for non-weapons.
    pub reach: f32,
    /// True when this weapon is a bow (WEAP `DNAM` animation
    /// type u32 @0 == 7 — archery probe: Hunting/Long/Imperial/
    /// Daedric/Falmer all 7, melee weapons 1-6). Bows fire
    /// arrows instead of swinging (see archery.rs). False for
    /// non-weapons.
    pub is_bow: bool,
    /// The in-flight arrow model (AMMO only): the PROJ record's
    /// MODL, resolved at catalog build through the AMMO's DATA
    /// @0 projectile form ID. `None` when the PROJ record is
    /// missing — the arrow cannot fly (archery logs it, it does
    /// not fake a model).
    pub projectile_model: Option<String>,
    /// Arrow speed in m/s (AMMO only): the PROJ DATA f32 @8
    /// (3600 u/s = 51.4 m/s for every arrow in Skyrim.esm).
    /// 0.0 when the PROJ record is missing.
    pub projectile_speed: f32,
    /// Arrow gravity multiplier (AMMO only): the PROJ DATA f32
    /// @4 (0.35 for every arrow) — multiplies the engine's
    /// gravity. 0.0 when the PROJ record is missing.
    pub projectile_gravity: f32,
    /// Armor rating (ARMO `DNAM` u32 / 100).
    pub armor_rating: f32,
    /// Where it is worn (ARMO only, from `BOD2`).
    pub slot: Option<EquipSlot>,
    /// The worn biped model for the PLAYER's race/sex (ARMO only,
    /// resolved at catalog build through the npc.rs chain).
    pub worn_model: Option<String>,
    /// The detail-pane description: a book's `CNAM` text
    /// (`.DLSTRINGS`, 33 books carry one). `None` for everything
    /// else — the game shows no description for weapons, armor,
    /// or clutter. Filled here in `parse_item_def`.
    pub description: Option<String>,
    /// A potion/food/ingredient's effect names (MGEF `FULL`s,
    /// in record order — "Restore Health"). Empty for
    /// everything else. Filled at catalog build (it needs the
    /// MGEF record index). Kept separate from `description` so
    /// a book's prose is never split on commas.
    pub effects: Vec<String>,
    /// The ground model (relative to `meshes/`), for drops: the
    /// base's `MODL` path for most kinds; ARMO uses the
    /// sex-matched ground model (`MOD2` male / `MOD4` female),
    /// resolved at catalog build. `None` for the 173/7355 item
    /// bases with no model (book_desc_probe) — those cannot be
    /// dropped visibly, so the drop refuses them instead of
    /// losing the item silently.
    pub ground_model: Option<String>,
}

/// Prettify an EDID into a display name: split CamelCase and
/// digit runs, capitalize the first letter. Mechanical and
/// predictable — the .strings lookup (M18) replaces it with the
/// real name.
pub fn prettify_edid(edid: &str) -> String {
    let chars: Vec<char> = edid.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 {
            let prev = chars[i - 1];
            // lowercase/digit -> uppercase, or letter -> digit, or
            // digit -> uppercase letter: a word boundary.
            let boundary = (prev.is_lowercase() && c.is_uppercase())
                || (prev.is_alphabetic() && c.is_ascii_digit())
                || (prev.is_ascii_digit() && c.is_uppercase());
            if boundary {
                out.push(' ');
            }
        }
        out.push(c);
    }
    if let Some(first) = out.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    out
}

/// Biped slot for a `BOD2` bitmask. Priority: head bits, then
/// body, hands, feet, shield — the first match is the item's
/// primary slot (a robe covering body+hands takes the body slot).
/// `None` when no known slot bit is set (rings, amulets — not
/// equipable in M17).
pub fn slot_from_bod2(slots: u32) -> Option<EquipSlot> {
    const HEAD_BITS: u32 = 0x0000_1003; // head 0x01, hair 0x02, circlet 0x1000
    if slots & HEAD_BITS != 0 {
        Some(EquipSlot::Head)
    } else if slots & 0x0000_0004 != 0 {
        Some(EquipSlot::Body)
    } else if slots & 0x0000_0008 != 0 {
        Some(EquipSlot::Hands)
    } else if slots & 0x0000_0080 != 0 {
        Some(EquipSlot::Feet)
    } else if slots & 0x0000_0200 != 0 {
        Some(EquipSlot::Shield)
    } else {
        None
    }
}

fn i32_at(data: &[u8], offset: usize) -> Option<i32> {
    data.get(offset..offset + 4)
        .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
}

fn f32_at(data: &[u8], offset: usize) -> Option<f32> {
    data.get(offset..offset + 4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
}

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    data.get(offset..offset + 2)
        .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    data.get(offset..offset + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// Parse one item base record into a definition. `None` when the
/// record is not an item type or has no EDID (it could not be
/// displayed). The worn model is filled in later by the catalog
/// build, which knows the player's race/sex.
///
/// M18: the name is the record's real name from `.strings`
/// (its `FULL` field) when `strings` is given and the ID
/// resolves — "Iron Sword", not "Iron Sword" by luck of the
/// EDID; "Sweetroll", not "Food Sweetroll". Otherwise the name
/// falls back to the prettified EDID (the M17 behaviour).
pub fn parse_item_def(
    record: &Record,
    strings: Option<&crate::dialogue::GameStrings>,
) -> Option<ItemDef> {
    let kind_code = record.type_str();
    let edid = record.edid()?;
    let data = record.field(b"DATA").map(|f| f.data.as_slice());
    let name = strings
        .and_then(|s| s.name_of(record))
        .unwrap_or_else(|| prettify_edid(&edid));
    let mut def = ItemDef {
        base_form_id: record.form_id,
        name,
        kind: ItemKind::Misc,
        value: 0,
        weight: 0.0,
        damage: 0,
        speed: 1.0,
        reach: 1.0,
        is_bow: false,
        projectile_model: None,
        projectile_speed: 0.0,
        projectile_gravity: 0.0,
        armor_rating: 0.0,
        slot: None,
        worn_model: None,
        description: None,
        effects: Vec::new(),
        ground_model: None,
    };
    // The ground model: the base's MODL path — except ARMO,
    // whose MODL is an ARMA form ID, not a path (its ground
    // models are MOD2/MOD4, resolved at catalog build where the
    // player's sex is known). MODL strings carry a trailing NUL.
    if kind_code != "ARMO" {
        def.ground_model = record.field(b"MODL").map(|f| {
            String::from_utf8_lossy(&f.data)
                .trim_end_matches('\0')
                .to_string()
        });
    }
    match kind_code.as_str() {
        "WEAP" => {
            def.kind = ItemKind::Weapon;
            let d = data?;
            def.value = i32_at(d, 0)?;
            def.weight = f32_at(d, 4)?;
            def.damage = u16_at(d, 8).unwrap_or(0);
            // Melee: speed + reach live in DNAM (100 bytes,
            // proven by the melee_probe against known vanilla
            // values — sword 1.0/1.0, dagger 1.3/0.7). Bows are
            // the animation type u32 @0 == 7 (archery probe:
            // all bows 7, melee weapons 1-6).
            if let Some(dnam) = record.field(b"DNAM") {
                def.speed = f32_at(&dnam.data, 4).unwrap_or(1.0);
                def.reach = f32_at(&dnam.data, 8).unwrap_or(1.0);
                def.is_bow = u32_at(&dnam.data, 0) == Some(7);
            }
        }
        "ARMO" => {
            def.kind = ItemKind::Armor;
            let d = data?;
            def.value = i32_at(d, 0)?;
            def.weight = f32_at(d, 4)?;
            if let Some(dnam) = record.field(b"DNAM") {
                def.armor_rating = u32_at(&dnam.data, 0).unwrap_or(0) as f32 / 100.0;
            }
            // SE armor uses BOD2; fall back to BODT if absent.
            let slots = record
                .field(b"BOD2")
                .and_then(|f| u32_at(&f.data, 0))
                .or_else(|| record.field(b"BODT").and_then(|f| u32_at(&f.data, 0)));
            def.slot = slots.and_then(slot_from_bod2);
        }
        "ALCH" => {
            // ALCH DATA is just the weight (f32); value and flags
            // live in ENIT (value u32, flags u32).
            let d = data?;
            def.weight = f32_at(d, 0)?;
            let enit = record.field(b"ENIT")?;
            def.value = u32_at(&enit.data, 0).unwrap_or(0) as i32;
            let flags = u32_at(&enit.data, 4).unwrap_or(0);
            def.kind = if flags & 0x01 != 0 {
                ItemKind::Food
            } else {
                ItemKind::Potion
            };
        }
        "BOOK" => {
            def.kind = ItemKind::Book;
            let d = data?;
            def.value = i32_at(d, 8)?;
            def.weight = f32_at(d, 12)?;
            // The detail-pane description (CNAM -> .DLSTRINGS);
            // None for most books, which carry no description.
            def.description = strings.and_then(|s| s.description_of(record));
        }
        "AMMO" => {
            def.kind = ItemKind::Ammo;
            // AMMO DATA is 20 bytes, NOT the WEAP 10-byte
            // layout (archery probe — the old u16@8 read gave
            // every arrow 0 damage): projectile form ID u32 @0,
            // unknown u32 @4 (4 for every arrow), damage f32 @8
            // (Iron 8, Steel 10, Daedric 24), value u32 @12
            // (Iron 1, Steel 2, Daedric 8), weight f32 @16
            // (0.1 for every arrow). The projectile resolves to
            // the flight model + speed at catalog build.
            if let Some(d) = data {
                def.damage = f32_at(d, 8).map(|v| v.round() as u16).unwrap_or(0);
                def.value = u32_at(d, 12).unwrap_or(0) as i32;
                def.weight = f32_at(d, 16).unwrap_or(0.0);
            }
        }
        "MISC" | "INGR" | "KEYM" | "SLGM" | "NOTE" | "SCRL" => {
            def.kind = match kind_code.as_str() {
                "INGR" => ItemKind::Ingredient,
                "KEYM" => ItemKind::Key,
                "SLGM" => ItemKind::SoulGem,
                "NOTE" => ItemKind::Note,
                "SCRL" => ItemKind::Scroll,
                _ => ItemKind::Misc,
            };
            // The 8-byte value/weight prefix.
            if let Some(d) = data {
                def.value = i32_at(d, 0).unwrap_or(0);
                def.weight = f32_at(d, 4).unwrap_or(0.0);
            }
        }
        _ => return None,
    }
    Some(def)
}

/// A potion/food's effect names, in record order: the ALCH's
/// `EFID` fields are MGEF form IDs, and each MGEF's `FULL`
/// resolves to its name in `.STRINGS` ("Restore Health").
/// Unresolvable effects are skipped, not shown as "?" — the
/// detail pane shows real effects or none. Pure — unit-tested.
pub fn alch_effect_names(
    record: &Record,
    by_form_id: &HashMap<u32, &Record>,
    strings: Option<&crate::dialogue::GameStrings>,
) -> Vec<String> {
    let Some(strings) = strings else {
        return Vec::new();
    };
    record
        .fields(b"EFID")
        .filter_map(|f| {
            if f.data.len() != 4 {
                return None;
            }
            let mgef_id = u32::from_le_bytes(f.data[0..4].try_into().unwrap());
            let mgef = by_form_id.get(&mgef_id)?;
            strings.name_of(mgef)
        })
        .collect()
}

/// One of the game's inventory categories, in the game's
/// order. The category filters the list; All shows everything.
/// Ten categories INCLUDING Scrolls (SCRL records have their
/// own category — Favorites is the Q menu, not a category).
/// Scrolls sits last: the .swf's category list is populated by
/// code, so the order is a judgment call, not a proven
/// position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InventoryTab {
    #[default]
    All,
    Weapons,
    Apparel,
    Potions,
    Food,
    Ingredients,
    Books,
    Keys,
    Misc,
    Scrolls,
}

impl InventoryTab {
    /// All ten categories, in the game's order.
    pub const ALL: [InventoryTab; 10] = [
        InventoryTab::All,
        InventoryTab::Weapons,
        InventoryTab::Apparel,
        InventoryTab::Potions,
        InventoryTab::Food,
        InventoryTab::Ingredients,
        InventoryTab::Books,
        InventoryTab::Keys,
        InventoryTab::Misc,
        InventoryTab::Scrolls,
    ];

    /// The tab's label, as the game shows it.
    pub fn label(self) -> &'static str {
        match self {
            InventoryTab::All => "ALL",
            InventoryTab::Weapons => "WEAPONS",
            InventoryTab::Apparel => "APPAREL",
            InventoryTab::Potions => "POTIONS",
            InventoryTab::Food => "FOOD",
            InventoryTab::Ingredients => "INGREDIENTS",
            InventoryTab::Books => "BOOKS",
            InventoryTab::Keys => "KEYS",
            InventoryTab::Misc => "MISC",
            InventoryTab::Scrolls => "SCROLLS",
        }
    }

    /// Parse a `--inventory-tab` name (lowercase, as typed on
    /// the command line). `None` for an unknown name — the
    /// caller falls back to All.
    pub fn from_name(name: &str) -> Option<InventoryTab> {
        match name.to_lowercase().as_str() {
            "all" => Some(InventoryTab::All),
            "weapons" => Some(InventoryTab::Weapons),
            "apparel" => Some(InventoryTab::Apparel),
            "potions" => Some(InventoryTab::Potions),
            "food" => Some(InventoryTab::Food),
            "ingredients" => Some(InventoryTab::Ingredients),
            "books" => Some(InventoryTab::Books),
            "keys" => Some(InventoryTab::Keys),
            "misc" => Some(InventoryTab::Misc),
            "scrolls" => Some(InventoryTab::Scrolls),
            _ => None,
        }
    }
}

/// Which tab an item kind belongs to: weapons and armor to
/// their tabs, potions and food split by the ALCH food flag
/// (decided at parse time), ingredients/books/keys/scrolls to
/// theirs, and everything else — ammo, soul gems, notes, misc
/// — under Misc, as the game files them.
pub fn tab_for_kind(kind: ItemKind) -> InventoryTab {
    match kind {
        ItemKind::Weapon => InventoryTab::Weapons,
        ItemKind::Armor => InventoryTab::Apparel,
        ItemKind::Potion => InventoryTab::Potions,
        ItemKind::Food => InventoryTab::Food,
        ItemKind::Ingredient => InventoryTab::Ingredients,
        ItemKind::Book => InventoryTab::Books,
        ItemKind::Key => InventoryTab::Keys,
        ItemKind::Scroll => InventoryTab::Scrolls,
        ItemKind::Ammo | ItemKind::Misc | ItemKind::SoulGem | ItemKind::Note => InventoryTab::Misc,
    }
}

/// The stacks visible under a tab, sorted by name (the game's
/// list order — alphabetical, case-insensitive). Stacks whose
/// def is missing from the catalog are skipped (they cannot be
/// displayed). Pure — unit-tested.
pub fn visible_stacks<'a>(
    inventory: &'a Inventory,
    catalog: &'a ItemCatalog,
    tab: InventoryTab,
) -> Vec<&'a InventoryStack> {
    let mut visible: Vec<&InventoryStack> = inventory
        .stacks
        .iter()
        .filter(|stack| {
            let Some(def) = catalog.def(stack.base_form_id) else {
                return false;
            };
            tab == InventoryTab::All || tab_for_kind(def.kind) == tab
        })
        .collect();
    visible.sort_by(|a, b| {
        let name_a = catalog.def(a.base_form_id).map(|d| d.name.as_str());
        let name_b = catalog.def(b.base_form_id).map(|d| d.name.as_str());
        name_a
            .unwrap_or("")
            .to_lowercase()
            .cmp(&name_b.unwrap_or("").to_lowercase())
    });
    visible
}

/// Whether the item card is the WIDE (185 px) variant: it
/// carries an effects or description block under the stats
/// (potion/food effects, ingredient effects, a book's
/// description). Everything else takes the SLIM (153 px) card.
/// Pure — unit-tested.
pub fn card_is_wide(def: &ItemDef) -> bool {
    !def.effects.is_empty() || def.description.is_some()
}

/// The bottom bar's primary-action label for one item (the
/// touch path for E): title case, as the game shows it — Equip
/// for wearables, Eat/Drink for consumables, nothing for
/// books/keys/clutter (reading is a later milestone). Pure —
/// unit-tested.
pub fn primary_action_label(def: &ItemDef) -> Option<&'static str> {
    match def.kind {
        ItemKind::Weapon | ItemKind::Armor => Some("Equip"),
        ItemKind::Food => Some("Eat"),
        ItemKind::Potion => Some("Drink"),
        _ => None,
    }
}

/// One stack in the inventory: a base form ID and a count.
/// Equipped items stay in their stack (Skyrim style) and are
/// marked by the equipment slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryStack {
    pub base_form_id: u32,
    pub count: i32,
}

/// The player's inventory: stacks, gold, and what is equipped.
#[derive(Debug, Default, Resource)]
pub struct Inventory {
    pub stacks: Vec<InventoryStack>,
    pub gold: u32,
    pub equipped_weapon: Option<u32>,
    pub equipped_armor: HashMap<EquipSlot, u32>,
    /// The starting outfit has been seeded (once per app run).
    pub seeded: bool,
}

/// What equipping did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipOutcome {
    Equipped,
    Unequipped,
    NotEquippable,
}

/// What using did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseOutcome {
    /// Consumed one (food/potion). The effect is a no-op until
    /// damage exists — health is full, so there is nothing to
    /// restore yet; the mechanic (count drops, log line) is real.
    Consumed,
    NotUsable,
}

impl Inventory {
    /// Add `count` of a base. Gold goes to the counter, never to
    /// a stack. Counts <= 0 are ignored.
    pub fn add(&mut self, base_form_id: u32, count: i32) {
        if count <= 0 {
            return;
        }
        if base_form_id == GOLD_FORM_ID {
            self.gold = self.gold.saturating_add(count as u32);
            return;
        }
        if let Some(stack) = self
            .stacks
            .iter_mut()
            .find(|s| s.base_form_id == base_form_id)
        {
            stack.count += count;
        } else {
            self.stacks.push(InventoryStack {
                base_form_id,
                count,
            });
        }
    }

    /// How many of a base the player carries (0 when absent).
    pub fn count(&self, base_form_id: u32) -> i32 {
        if base_form_id == GOLD_FORM_ID {
            return self.gold as i32;
        }
        self.stacks
            .iter()
            .find(|s| s.base_form_id == base_form_id)
            .map(|s| s.count)
            .unwrap_or(0)
    }

    /// Remove `count` of a base. Returns false (and removes
    /// nothing) when the player carries fewer. An emptied stack
    /// disappears; an equipped item that hits zero unequips.
    pub fn remove(&mut self, base_form_id: u32, count: i32) -> bool {
        if count <= 0 {
            return false;
        }
        if base_form_id == GOLD_FORM_ID {
            if self.gold < count as u32 {
                return false;
            }
            self.gold -= count as u32;
            return true;
        }
        let Some(stack) = self
            .stacks
            .iter_mut()
            .find(|s| s.base_form_id == base_form_id)
        else {
            return false;
        };
        if stack.count < count {
            return false;
        }
        stack.count -= count;
        if stack.count == 0 {
            self.stacks.retain(|s| s.base_form_id != base_form_id);
            if self.equipped_weapon == Some(base_form_id) {
                self.equipped_weapon = None;
            }
            self.equipped_armor.retain(|_, v| *v != base_form_id);
        }
        true
    }

    /// True when the base is currently equipped (weapon or any
    /// armor slot).
    pub fn is_equipped(&self, base_form_id: u32) -> bool {
        self.equipped_weapon == Some(base_form_id)
            || self.equipped_armor.values().any(|v| *v == base_form_id)
    }

    /// Equip a carried item: weapons go to the weapon slot, armor
    /// to its BOD2 slot (replacing whatever was there). Equipping
    /// an already-equipped item unequips it. Returns
    /// `NotEquippable` for non-wearable kinds, slotless armor
    /// (rings/amulets — M17 has no slots for them), and items the
    /// player does not carry.
    pub fn equip(&mut self, base_form_id: u32, def: &ItemDef) -> EquipOutcome {
        if self.count(base_form_id) == 0 {
            return EquipOutcome::NotEquippable;
        }
        match def.kind {
            ItemKind::Weapon => {
                if self.equipped_weapon == Some(base_form_id) {
                    self.equipped_weapon = None;
                    EquipOutcome::Unequipped
                } else {
                    self.equipped_weapon = Some(base_form_id);
                    EquipOutcome::Equipped
                }
            }
            ItemKind::Armor => {
                let Some(slot) = def.slot else {
                    return EquipOutcome::NotEquippable;
                };
                if self.equipped_armor.get(&slot) == Some(&base_form_id) {
                    self.equipped_armor.remove(&slot);
                    EquipOutcome::Unequipped
                } else {
                    self.equipped_armor.insert(slot, base_form_id);
                    EquipOutcome::Equipped
                }
            }
            _ => EquipOutcome::NotEquippable,
        }
    }

    /// Use a carried item: food and potions are consumed (one
    /// count). Everything else is not usable in M17 (books are
    /// read in a later milestone).
    pub fn use_item(&mut self, base_form_id: u32, def: &ItemDef) -> UseOutcome {
        match def.kind {
            ItemKind::Food | ItemKind::Potion => {
                if self.remove(base_form_id, 1) {
                    UseOutcome::Consumed
                } else {
                    UseOutcome::NotUsable
                }
            }
            _ => UseOutcome::NotUsable,
        }
    }

    /// Total carried weight (gold weighs nothing).
    pub fn total_weight(&self, catalog: &ItemCatalog) -> f32 {
        self.stacks
            .iter()
            .map(|s| {
                catalog
                    .def(s.base_form_id)
                    .map(|d| d.weight * s.count as f32)
                    .unwrap_or(0.0)
            })
            .sum()
    }

    /// Total carried value. Unused since the redesign dropped
    /// the total-value line from the screen (the game does not
    /// show it) — kept for the trading milestone, which prices
    /// the player's goods.
    #[allow(dead_code)]
    pub fn total_value(&self, catalog: &ItemCatalog) -> i32 {
        self.stacks
            .iter()
            .map(|s| {
                catalog
                    .def(s.base_form_id)
                    .map(|d| d.value * s.count)
                    .unwrap_or(0)
            })
            .sum()
    }
}

/// One leveled-list entry: the minimum player level, the item (or
/// nested list) form ID, and the count.
#[derive(Debug, Clone, Copy)]
pub struct LvliEntry {
    pub level: i16,
    pub item_form_id: u32,
    pub count: i16,
}

/// Per-list LVLI metadata the entry table does not carry: the
/// LVLF flags byte (bit 0x04 = Use All) and the LVLD Chance
/// None (0-100). Read at catalog build; the outfit resolver
/// needs both (npc.rs reads them from the record directly).
#[derive(Debug, Clone, Copy, Default)]
pub struct LvliMeta {
    pub flags: u8,
    pub chance_none: u8,
}

/// Everything the inventory, pickup, container, and player-model
/// systems need from the plugin, parsed once into owned data.
#[derive(Debug, Default, Resource)]
pub struct ItemCatalog {
    /// Item definitions by base form ID.
    pub defs: HashMap<u32, ItemDef>,
    /// Leveled lists by form ID.
    pub lvli: HashMap<u32, Vec<LvliEntry>>,
    /// Leveled-list metadata by form ID (LVLF flags + LVLD
    /// Chance None) — the outfit resolver's flag rule needs
    /// what the entry table does not carry.
    pub lvli_meta: HashMap<u32, LvliMeta>,
    /// Container contents by CONT base form ID: (item form ID,
    /// count) — the raw CNTO, LVLIs included (resolved at
    /// take-time).
    pub container_contents: HashMap<u32, Vec<(u32, i32)>>,
    /// Flora harvest yields by FLOR base form ID: the PFIG form
    /// ID — the item (ALCH/INGR) or leveled list (coin purses)
    /// a harvest gives. A FLOR ref gives its PFIG yield, not
    /// itself: a hanging rabbit becomes FoodRabbit, a coin
    /// purse becomes gold from its LVLI. (clutter_probe,
    /// 2026-11-15: all 10 FLOR bases in the inn carry PFIG.)
    pub flor_yield: HashMap<u32, u32>,
    /// The player's starting outfit (NPC_ 0x00000007's DOFT ->
    /// OTFT -> ARMO form IDs), seeded into the inventory once.
    pub starting_outfit: Vec<u32>,
    /// The player's bare body models (NakedTorso/Hands/Feet for
    /// the player's race/sex).
    pub player_body: Vec<String>,
    /// The player's head model (`MaleHead.nif` / `FemaleHead.nif`
    /// — the player has no FaceGen; the generic head is the
    /// stand-in until character creation exists).
    pub player_head: String,
    /// The player's race/sex (from NPC_ 0x00000007). Read at
    /// build time for the body/head/armor resolution; kept on the
    /// catalog for the milestones that need them (dialogue race
    /// checks, character creation) rather than re-parsed.
    #[allow(dead_code)]
    pub player_race: u32,
    #[allow(dead_code)]
    pub player_female: bool,
    /// Built once per app run (the build walks all ~870k records).
    pub built: bool,
}

impl ItemCatalog {
    pub fn def(&self, base_form_id: u32) -> Option<&ItemDef> {
        self.defs.get(&base_form_id)
    }

    /// Build the catalog from the parsed plugin: one pass over
    /// all records. Item defs, leveled lists, container contents,
    /// the player's starting outfit, and the player's body/head.
    /// `strings` (M18) resolves real item names; `None` keeps
    /// the prettified-EDID names.
    pub fn build(plugin: &EsmPlugin, strings: Option<&crate::dialogue::GameStrings>) -> Self {
        let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
        for record in plugin.all_records() {
            by_form_id.insert(record.form_id, record);
        }
        // The player: race/sex/outfit from NPC_ 0x00000007.
        let (player_race, player_female, outfit_id) = match by_form_id.get(&PLAYER_FORM_ID) {
            Some(npc) => {
                let female = npc
                    .field(b"ACBS")
                    .and_then(|f| f.data.first().map(|b| b & 1 == 1))
                    .unwrap_or(false);
                let race = npc
                    .field(b"RNAM")
                    .and_then(|f| {
                        if f.data.len() == 4 {
                            Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
                        } else {
                            None
                        }
                    })
                    .unwrap_or(npc::DEFAULT_RACE_FORM_ID);
                let outfit = npc.field(b"DOFT").and_then(|f| {
                    if f.data.len() == 4 {
                        Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
                    } else {
                        None
                    }
                });
                (race, female, outfit)
            }
            // No player record (a plugin without one): Nord male,
            // the same default the game ships.
            None => (0x0001_3746, false, None),
        };
        // The outfit itself expands AFTER the main loop below —
        // its entries can be leveled lists (the player's default
        // outfit, ArmorIronAllOutfit, points at ArmorIronSetAll),
        // and the LVLI map does not exist until the loop runs.
        let npc_index = npc::NpcIndex::build(&by_form_id);
        let player_body = npc::body_parts(&npc_index, player_race, player_female);
        let player_head = if player_female {
            "Actors\\Character\\Character Assets\\FemaleHead.nif".to_string()
        } else {
            "Actors\\Character\\Character Assets\\MaleHead.nif".to_string()
        };

        let mut catalog = ItemCatalog {
            defs: HashMap::new(),
            lvli: HashMap::new(),
            lvli_meta: HashMap::new(),
            container_contents: HashMap::new(),
            flor_yield: HashMap::new(),
            starting_outfit: Vec::new(),
            player_body,
            player_head,
            player_race,
            player_female,
            built: true,
        };
        for record in plugin.all_records() {
            match record.type_str().as_str() {
                "LVLI" => {
                    let mut entries = Vec::new();
                    for f in record.fields(b"LVLO") {
                        if f.data.len() != 12 {
                            continue;
                        }
                        let level = i16::from_le_bytes(f.data[0..2].try_into().unwrap());
                        let item_form_id = u32::from_le_bytes(f.data[4..8].try_into().unwrap());
                        let count = i16::from_le_bytes(f.data[8..10].try_into().unwrap());
                        entries.push(LvliEntry {
                            level,
                            item_form_id,
                            count,
                        });
                    }
                    catalog.lvli.insert(record.form_id, entries);
                    let flags = record
                        .field(b"LVLF")
                        .and_then(|f| f.data.first())
                        .copied()
                        .unwrap_or(0);
                    let chance_none = record
                        .field(b"LVLD")
                        .and_then(|f| f.data.first())
                        .copied()
                        .unwrap_or(0);
                    catalog
                        .lvli_meta
                        .insert(record.form_id, LvliMeta { flags, chance_none });
                }
                "CONT" => {
                    let mut contents = Vec::new();
                    for f in record.fields(b"CNTO") {
                        if f.data.len() != 8 {
                            continue;
                        }
                        let item_id = u32::from_le_bytes(f.data[0..4].try_into().unwrap());
                        let count = i32::from_le_bytes(f.data[4..8].try_into().unwrap());
                        contents.push((item_id, count));
                    }
                    if !contents.is_empty() {
                        catalog.container_contents.insert(record.form_id, contents);
                    }
                }
                "FLOR" => {
                    // The harvest yield: PFIG is a 4-byte form
                    // ID (direct item or leveled list). A FLOR
                    // without PFIG yields nothing — skipped.
                    if let Some(pfig) = record.field(b"PFIG") {
                        if pfig.data.len() == 4 {
                            let yield_id = u32::from_le_bytes(pfig.data[0..4].try_into().unwrap());
                            catalog.flor_yield.insert(record.form_id, yield_id);
                        }
                    }
                }
                _ => {
                    if let Some(mut def) = parse_item_def(record, strings) {
                        // ARMO: resolve the worn model for the
                        // player's race/sex now, through the
                        // npc.rs chain — the player model reads
                        // it straight from the def. The ground
                        // model is the sex-matched MOD2/MOD4
                        // (MOD2 male, MOD4 female), falling back
                        // to the other when the matched one is
                        // missing — a dropped cuirass shows
                        // whichever ground model the record has.
                        if def.kind == ItemKind::Armor {
                            def.worn_model = npc::armor_model(
                                &by_form_id,
                                record.form_id,
                                player_race,
                                player_female,
                            );
                            let (first, second) = if player_female {
                                (b"MOD4", b"MOD2")
                            } else {
                                (b"MOD2", b"MOD4")
                            };
                            let ground =
                                record
                                    .field(first)
                                    .or_else(|| record.field(second))
                                    .map(|f| {
                                        String::from_utf8_lossy(&f.data)
                                            .trim_end_matches('\0')
                                            .to_string()
                                    });
                            def.ground_model = ground;
                        }
                        // ALCH + INGR: the card's effect names
                        // (MGEF FULLs, resolved through the
                        // record index + strings). Ingredients
                        // carry the same EFID fields as potions
                        // (up to 4) — the card shows them in its
                        // 2x2 grid.
                        if matches!(
                            def.kind,
                            ItemKind::Food | ItemKind::Potion | ItemKind::Ingredient
                        ) {
                            def.effects = alch_effect_names(record, &by_form_id, strings);
                        }
                        // AMMO: resolve the projectile (PROJ)
                        // record through the AMMO's DATA @0 form
                        // ID — the flight model (PROJ MODL) and
                        // the flight parameters (PROJ DATA @8
                        // speed in u/s, @4 gravity multiplier).
                        // A missing PROJ leaves the fields empty
                        // and the arrow cannot fly (archery logs
                        // it instead of faking a model).
                        if def.kind == ItemKind::Ammo {
                            if let Some(proj_id) =
                                record.field(b"DATA").and_then(|f| u32_at(&f.data, 0))
                            {
                                if let Some(proj) = by_form_id.get(&proj_id) {
                                    def.projectile_model = proj.field(b"MODL").map(|f| {
                                        String::from_utf8_lossy(&f.data)
                                            .trim_end_matches('\0')
                                            .to_string()
                                    });
                                    if let Some(d) = proj.field(b"DATA") {
                                        def.projectile_speed = f32_at(&d.data, 8)
                                            .map(|v| v / crate::coords::SKYRIM_UNITS_PER_METER)
                                            .unwrap_or(0.0);
                                        def.projectile_gravity = f32_at(&d.data, 4).unwrap_or(0.0);
                                    }
                                }
                            }
                        }
                        catalog.defs.insert(record.form_id, def);
                    }
                }
            }
        }
        // The starting outfit, expanded now that the LVLI map
        // exists: each OTFT entry is either an ARMO (taken
        // directly) or a leveled list (resolved by its LVLF
        // flag — Use All takes the whole set, a pick-one list
        // takes one deterministic piece; the player's default
        // outfit is ArmorIronSetAll, 4 pieces). Non-armor
        // entries are dropped: an outfit is worn, not carried.
        if let Some(outfit_id) = outfit_id {
            if let Some(outfit) = by_form_id.get(&outfit_id) {
                for inam in outfit.fields(b"INAM") {
                    for chunk in inam.data.as_chunks::<4>().0 {
                        let entry_id = u32::from_le_bytes(*chunk);
                        let Some(entry) = by_form_id.get(&entry_id) else {
                            continue;
                        };
                        match entry.type_str().as_str() {
                            "ARMO" => catalog.starting_outfit.push(entry_id),
                            "LVLI" => {
                                for id in resolve_outfit_lvli(&catalog, entry_id, 1, 0) {
                                    if by_form_id
                                        .get(&id)
                                        .map(|r| r.type_str() == "ARMO")
                                        .unwrap_or(false)
                                    {
                                        catalog.starting_outfit.push(id);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        catalog
    }
}

/// Resolve an outfit's leveled list to item form IDs, taking ALL
/// entries at or below the player level (an outfit is a set —
/// `ArmorIronSetAll` is boots + cuirass + gauntlets + helmet,
/// and the player wears all four). Recurses into nested lists
/// (depth cap 4). Player level 1 until progression exists.
/// Resolve an outfit's leveled list to item form IDs, by the
/// LVLF flags (mirrors npc.rs's version, over the catalog's
/// parsed tables instead of the records):
///
/// - Use All (0x04): EVERY entry at or below the player level
///   (an outfit set — the player's ArmorIronSetAll). Nested
///   lists recurse and apply their OWN flag.
/// - Otherwise: ONE deterministic pick among the eligible
///   entries (the M17 container rule with no container seed —
///   the player is the only wearer, so the same list always
///   resolves the same way). Chance None (LVLD) is honored
///   here: the roll may yield nothing. Use-All lists ignore
///   chance.
///
/// Depth cap 4 (M17).
fn resolve_outfit_lvli(
    catalog: &ItemCatalog,
    lvli_id: u32,
    player_level: i16,
    depth: u32,
) -> Vec<u32> {
    if depth > 4 {
        return Vec::new();
    }
    let Some(entries) = catalog.lvli.get(&lvli_id) else {
        return Vec::new();
    };
    let eligible: Vec<&LvliEntry> = entries.iter().filter(|e| e.level <= player_level).collect();
    if eligible.is_empty() {
        return Vec::new();
    }
    let meta = catalog.lvli_meta.get(&lvli_id).copied().unwrap_or_default();
    if meta.flags & npc::LVLI_FLAG_USE_ALL != 0 {
        let mut out = Vec::new();
        for entry in eligible {
            if catalog.lvli.contains_key(&entry.item_form_id) {
                out.extend(resolve_outfit_lvli(
                    catalog,
                    entry.item_form_id,
                    player_level,
                    depth + 1,
                ));
            } else {
                out.push(entry.item_form_id);
            }
        }
        return out;
    }
    // Pick-one: the deterministic hash, mirroring M17's
    // `resolve_lvli` (no container seed — the player is the
    // only wearer).
    let hash = lvli_id
        .wrapping_mul(2_654_435_761)
        .wrapping_add(depth.wrapping_mul(97));
    if hash % 100 < meta.chance_none as u32 {
        return Vec::new();
    }
    let picked = eligible[(hash as usize) % eligible.len()];
    if catalog.lvli.contains_key(&picked.item_form_id) {
        resolve_outfit_lvli(catalog, picked.item_form_id, player_level, depth + 1)
    } else {
        vec![picked.item_form_id]
    }
}

/// Resolve a leveled list to concrete items, DETERMINISTICALLY
/// (the M17 approximation — see the module doc): among the
/// entries at or below the player level, pick one by a hash of
/// (list, container), then recurse into nested lists (depth cap
/// 4). `player_level` is the character's level (1 until
/// progression exists); `container_ref` is the ref form ID of
/// the container being looted (0 for a bare list lookup).
pub fn resolve_lvli(
    catalog: &ItemCatalog,
    lvli_id: u32,
    player_level: i16,
    container_ref: u32,
) -> Vec<(u32, i32)> {
    resolve_lvli_depth(catalog, lvli_id, player_level, container_ref, 0)
}

fn resolve_lvli_depth(
    catalog: &ItemCatalog,
    lvli_id: u32,
    player_level: i16,
    container_ref: u32,
    depth: u32,
) -> Vec<(u32, i32)> {
    if depth > 4 {
        return Vec::new();
    }
    let Some(entries) = catalog.lvli.get(&lvli_id) else {
        return Vec::new();
    };
    let eligible: Vec<&LvliEntry> = entries.iter().filter(|e| e.level <= player_level).collect();
    if eligible.is_empty() {
        return Vec::new();
    }
    // Deterministic pick: the same (list, container) always
    // resolves the same way; different containers vary.
    let hash = lvli_id
        .wrapping_mul(2_654_435_761)
        .wrapping_add(container_ref.wrapping_mul(40_503))
        .wrapping_add(depth.wrapping_mul(97));
    let picked = eligible[(hash as usize) % eligible.len()];
    // A nested list resolves through the same rule (its count
    // multiplies the inner result).
    if catalog.lvli.contains_key(&picked.item_form_id) {
        let inner = resolve_lvli_depth(
            catalog,
            picked.item_form_id,
            player_level,
            container_ref,
            depth + 1,
        );
        inner
            .into_iter()
            .map(|(id, n)| (id, n * picked.count as i32))
            .collect()
    } else {
        vec![(picked.item_form_id, picked.count as i32)]
    }
}

/// Whether the inventory screen is open. The player controller
/// gates movement/look on this (M17): the world pauses while the
/// bag is open, as in the game.
#[derive(Debug, Default, Resource)]
pub struct InventoryOpen(pub bool);

/// A cross-menu request to open or close the inventory (UI
/// milestone 2): the character menu's Items tab opens the
/// inventory, and the MAP button hands off from the inventory
/// back to the character menu. `open_inventory_ui` /
/// `close_inventory_ui` take many parameters, so the request
/// keeps them inside this module — the consumer below owns the
/// parameter list. Consumed (reset to `None`) by
/// `apply_inventory_requests` in the same frame it is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryAction {
    Open,
    Close,
}

/// The pending inventory open/close request, if any.
#[derive(Resource, Default)]
pub struct InventoryRequest(pub Option<InventoryAction>);

/// The selected item in the list (by base form ID), for
/// equip/use/drop and the detail pane. Survives UI rebuilds.
#[derive(Debug, Default, Resource)]
pub struct SelectedItem(pub Option<u32>);

/// The selected category tab. Survives UI rebuilds.
#[derive(Debug, Default, Resource)]
pub struct SelectedTab(pub InventoryTab);

/// Which list the arrows drive: the category list or the item
/// list (vanilla: Left/Right switch focus between them).
/// Defaults to Items — opening selects the first item, and the
/// arrows move through it.
#[derive(Debug, Default, Resource, Clone, Copy, PartialEq, Eq)]
pub enum InventoryFocus {
    Categories,
    #[default]
    Items,
}

/// The vanilla menu stage is 1280x720 (the .swf stage);
/// `UiScale` is the window height / 720, so the menu keeps its
/// proportions at any resolution. The screenshot path sets it
/// from the capture target (no window exists there);
/// `update_ui_scale` refreshes it from the primary window
/// every frame in the windowed app (a resize re-scales the
/// menu on its next rebuild). The dialogue menu reads this too
/// — it is the shared menu scale.
#[derive(Debug, Resource)]
pub struct UiScale(pub f32);

impl Default for UiScale {
    fn default() -> Self {
        UiScale(1.0)
    }
}

/// The game's key icons (E / R / Tab), loaded at runtime from
/// Interface.bsa (`interface/exported/*.png.dds`) — the same
/// rule as the HUD art: the game's own textures from the
/// player's copy, never bundled. Loaded once, on the first
/// inventory open (or by `ensure_interface_icons` when a
/// dialogue opens first); `loaded` stops a retry when
/// Interface.bsa is missing (the buttons fall back to text).
#[derive(Debug, Default, Resource)]
pub struct InterfaceIcons {
    pub e: Option<Handle<Image>>,
    pub r: Option<Handle<Image>>,
    pub tab: Option<Handle<Image>>,
    pub loaded: bool,
}

/// The doll needs a rebuild (an equip/unequip/drop changed what
/// the player wears). Set by the mutation helpers, consumed by
/// `rebuild_doll_if_dirty` — the rebuild is deferred so a
/// single frame's worth of changes costs one respawn. The
/// inventory no longer shows the doll, so the flag sits set
/// until a screen that shows the doll consumes it.
#[derive(Debug, Default, Resource)]
pub struct DollDirty(pub bool);

/// The next synthetic ref form ID for a dropped item. Dropped
/// items have no ESM ref, but the pickup index is keyed by ref
/// form ID — so drops take IDs from the 0xFF00_0000 block,
/// counting down. Plugin index 0xFF never occurs in the load
/// order (five plugins, indices 0–4), so a synthetic ID can
/// never collide with a real ref.
#[derive(Debug, Resource)]
pub struct DroppedRefIds(pub u32);

impl Default for DroppedRefIds {
    fn default() -> Self {
        DroppedRefIds(0xFF00_0000)
    }
}

/// Screenshot-proof seed (`--give` / `--inventory` /
/// `--inventory-tab`): items to add to the inventory (wearables
/// auto-equip — proof-hook behavior), whether to open the
/// inventory UI, and which tab to open it on. Inserted only on
/// the screenshot path; absent everywhere else.
#[derive(Debug, Resource)]
pub struct InventorySeed {
    pub items: Vec<u32>,
    pub open_inventory: bool,
    pub tab: Option<InventoryTab>,
}

/// Marker for the inventory UI root (despawned on close).
#[derive(Component)]
pub struct InventoryUiRoot;

/// Marker for the scrollable item list (its `ScrollPosition`
/// is the centered-list offset, computed from the selection at
/// build time).
#[derive(Component)]
struct InventoryList;

/// Marker for one item row in the list.
#[derive(Component)]
struct InventoryRow {
    base_form_id: u32,
}

/// Marker for one category row in the category list.
#[derive(Component)]
struct InventoryCategoryRow {
    tab: InventoryTab,
}

/// Marker for one bottom-bar button: the primary action (the
/// touch path for E), Drop (the touch path for R), or Close
/// (the touch path for Tab).
#[derive(Component)]
struct InventoryBarButton {
    action: BarAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarAction {
    Primary,
    Drop,
    Close,
}

/// One bottom-bar button's spec: the action, the key icon
/// (the game's texture, when Interface.bsa provided it), the
/// key's fallback text, the label (`None` hides the button),
/// and the fixed x slot (78/168/258 — a hidden button leaves
/// its slot empty, it does not shift the others).
struct BarButtonSpec {
    action: BarAction,
    label: Option<&'static str>,
    x: f32,
}

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Inventory>()
            .init_resource::<ItemCatalog>()
            .init_resource::<InventoryOpen>()
            .init_resource::<SelectedItem>()
            .init_resource::<SelectedTab>()
            .init_resource::<InventoryFocus>()
            .init_resource::<UiScale>()
            .init_resource::<InterfaceIcons>()
            .init_resource::<DollDirty>()
            .init_resource::<DroppedRefIds>()
            .init_resource::<InventoryRequest>()
            .init_resource::<crate::item_model::ItemModelYaw>()
            .init_resource::<crate::item_model::ItemModelSelection>()
            .add_systems(
                Startup,
                (
                    crate::player_model::create_doll_target,
                    crate::item_model::create_item_model_target,
                ),
            )
            .add_systems(
                Update,
                (
                    update_ui_scale,
                    ensure_interface_icons,
                    // UI milestone 1: the inventory does not
                    // open while the pause screen is open (the
                    // toggle is already at Bevy's 16-parameter
                    // limit, so the gate is a run condition,
                    // not a 17th parameter).
                    toggle_inventory.run_if(pause_closed),
                    apply_inventory_requests,
                    log_bag_press_outcome,
                    refresh_inventory_ui,
                    inventory_keys,
                    inventory_drop_key,
                    rebuild_doll_if_dirty,
                    crate::item_model::spawn_item_model_on_open,
                    crate::item_model::rebuild_item_model_on_selection,
                    crate::item_model::apply_item_model_yaw,
                ),
            );
    }
}

/// Run condition: the pause screen is closed (or the pause
/// plugin is not registered — the unit tests' minimal app has
/// no pause screen). UI milestone 2: the character menu must be
/// closed too — BAG must not open the inventory over it (the
/// Items tab is the only way in from the character menu).
fn pause_closed(
    pause_open: Option<Res<crate::pause::PauseOpen>>,
    character_open: Option<Res<crate::character::CharacterMenuOpen>>,
) -> bool {
    !pause_open.map(|o| o.0).unwrap_or(false) && !character_open.map(|o| o.0).unwrap_or(false)
}

/// Refresh the menu scale from the primary window (height /
/// 720). No-op on the screenshot path (no window — the seed
/// sets the scale from the capture target instead).
fn update_ui_scale(windows: Query<&Window, With<PrimaryWindow>>, mut scale: ResMut<UiScale>) {
    let Ok(window) = windows.single() else {
        return;
    };
    let next = window.height() / STAGE_HEIGHT;
    if (next - scale.0).abs() > 0.001 {
        scale.0 = next;
    }
}

/// Build the item catalog (once per app run) and seed the
/// player's starting outfit (once): the DOFT outfit's pieces go
/// into the inventory and onto the character — the player's own
/// clothes from the game's record, not a handout. Runs in the
/// scene-load chain (after the plugin resource exists); the
/// `built` / `seeded` flags make re-runs on door transitions
/// no-ops.
pub fn build_catalog(
    plugin: Option<Res<crate::SharedPlugin>>,
    strings: Option<Res<crate::dialogue::GameStrings>>,
    mut catalog: ResMut<ItemCatalog>,
    mut inventory: ResMut<Inventory>,
) {
    let Some(plugin) = plugin else {
        return;
    };
    if !catalog.built {
        let started = std::time::Instant::now();
        *catalog = ItemCatalog::build(&plugin.0, strings.as_deref());
        crate::startup_log::log(&format!(
            "item catalog built: {} defs, {} leveled lists, {} containers, {} flora yields, starting outfit {} pieces ({} ms)",
            catalog.defs.len(),
            catalog.lvli.len(),
            catalog.container_contents.len(),
            catalog.flor_yield.len(),
            catalog.starting_outfit.len(),
            started.elapsed().as_millis()
        ));
    }
    if !inventory.seeded {
        inventory.seeded = true;
        for armo_id in catalog.starting_outfit.clone() {
            inventory.add(armo_id, 1);
            if let Some(def) = catalog.def(armo_id) {
                inventory.equip(armo_id, def);
            }
        }
        if !catalog.starting_outfit.is_empty() {
            crate::startup_log::log(&format!(
                "starting outfit seeded: {} pieces equipped",
                catalog.starting_outfit.len()
            ));
        }
    }
}

/// Apply the screenshot-proof seed (`--give` / `--inventory` /
/// `--inventory-tab`): add the items (wearables auto-equip),
/// select the seeded tab, and open the inventory UI when asked.
/// No-op without the seed resource. Runs in the scene-load
/// chain, after `build_catalog`.
#[allow(clippy::too_many_arguments)]
pub fn apply_inventory_seed(
    mut commands: Commands,
    seed: Option<Res<InventorySeed>>,
    mut images: ResMut<Assets<Image>>,
    mut assets: Option<ResMut<crate::assets::AssetProvider>>,
    catalog: Res<ItemCatalog>,
    mut inventory: ResMut<Inventory>,
    mut open: ResMut<InventoryOpen>,
    mut selected: ResMut<SelectedItem>,
    mut selected_tab: ResMut<SelectedTab>,
    mut focus: ResMut<InventoryFocus>,
    mut scale: ResMut<UiScale>,
    mut icons: ResMut<InterfaceIcons>,
    item_target: Option<Res<crate::item_model::ItemModelTarget>>,
    screenshot_target: Option<Res<crate::screenshot::ScreenshotTarget>>,
) {
    let Some(seed) = seed else {
        return;
    };
    // The menu scale from the capture target (no window exists
    // on the screenshot path): the target's height / 720.
    if let Some(target) = screenshot_target.as_deref() {
        if let Some(image) = images.get(&target.image) {
            scale.0 = image.height() as f32 / STAGE_HEIGHT;
        }
    }
    for base_form_id in &seed.items {
        inventory.add(*base_form_id, 1);
        if let Some(def) = catalog.def(*base_form_id) {
            // Equip only when not already equipped — equip()
            // toggles, and a `--give` piece that is already on
            // (the starting outfit) would come OFF.
            if matches!(def.kind, ItemKind::Weapon | ItemKind::Armor)
                && !inventory.is_equipped(*base_form_id)
            {
                inventory.equip(*base_form_id, def);
            }
        }
    }
    if !seed.items.is_empty() {
        crate::startup_log::log(&format!(
            "inventory seed applied: {} items (--give)",
            seed.items.len()
        ));
    }
    if let Some(tab) = seed.tab {
        selected_tab.0 = tab;
    }
    if seed.open_inventory {
        open_inventory_ui(
            &mut commands,
            &mut images,
            assets.as_deref_mut(),
            &catalog,
            &inventory,
            &mut open,
            &mut selected,
            &selected_tab,
            &mut focus,
            scale.0,
            item_target.as_deref(),
            &mut icons,
        );
    }
    commands.remove_resource::<InventorySeed>();
}

/// Open the inventory: spawn the UI (the item-model rig spawns
/// in Update — `spawn_item_model_on_open` — the first frame
/// the inventory is open). Shared by the toggle system and the
/// screenshot seed. The first visible item is selected, as in
/// the game — the card is never empty on open when the tab has
/// items. The key icons load on the first open (see
/// `InterfaceIcons`).
#[allow(clippy::too_many_arguments)]
fn open_inventory_ui(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    assets: Option<&mut crate::assets::AssetProvider>,
    catalog: &ItemCatalog,
    inventory: &Inventory,
    open: &mut InventoryOpen,
    selected: &mut SelectedItem,
    selected_tab: &SelectedTab,
    focus: &mut InventoryFocus,
    scale: f32,
    item_target: Option<&crate::item_model::ItemModelTarget>,
    icons: &mut InterfaceIcons,
) {
    if open.0 {
        return;
    }
    open.0 = true;
    *focus = InventoryFocus::Items;
    selected.0 = visible_stacks(inventory, catalog, selected_tab.0)
        .first()
        .map(|s| s.base_form_id);
    if !icons.loaded {
        if let Some(assets) = assets {
            *icons = load_interface_icons(images, assets);
        }
    }
    spawn_inventory_ui(
        commands,
        catalog,
        inventory,
        selected,
        selected_tab,
        focus,
        scale,
        item_target,
        icons,
    );
    crate::startup_log::log("inventory opened");
}

/// Close the inventory: despawn the UI and the item-model rig.
/// (The doll is not shown in the inventory — nothing to
/// despawn for it.)
fn close_inventory_ui(
    commands: &mut Commands,
    ui_roots: &Query<Entity, With<InventoryUiRoot>>,
    item_model_entities: &Query<Entity, With<crate::item_model::ItemModelEntity>>,
    open: &mut InventoryOpen,
) {
    if !open.0 {
        return;
    }
    open.0 = false;
    for entity in ui_roots.iter() {
        commands.entity(entity).despawn();
    }
    for entity in item_model_entities.iter() {
        commands.entity(entity).despawn();
    }
    crate::startup_log::log("inventory closed");
}

/// Toggle the inventory on the BAG button (touch) or B (keyboard).
/// Opening spawns the UI (the item-model rig follows in
/// Update); closing despawns them both.
#[allow(clippy::too_many_arguments)]
fn toggle_inventory(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    touch: Option<Res<crate::touch::TouchInputState>>,
    mut images: ResMut<Assets<Image>>,
    mut assets: Option<ResMut<crate::assets::AssetProvider>>,
    catalog: Res<ItemCatalog>,
    inventory: ResMut<Inventory>,
    mut open: ResMut<InventoryOpen>,
    mut selected: ResMut<SelectedItem>,
    selected_tab: Res<SelectedTab>,
    mut focus: ResMut<InventoryFocus>,
    scale: Res<UiScale>,
    item_target: Option<Res<crate::item_model::ItemModelTarget>>,
    mut icons: ResMut<InterfaceIcons>,
    ui_roots: Query<Entity, With<InventoryUiRoot>>,
    item_model_entities: Query<Entity, With<crate::item_model::ItemModelEntity>>,
) {
    let bag_pressed = keyboard.just_pressed(KeyCode::KeyB)
        || touch.as_deref().map(|t| t.bag_pressed).unwrap_or(false);
    if !bag_pressed {
        return;
    }
    if open.0 {
        close_inventory_ui(&mut commands, &ui_roots, &item_model_entities, &mut open);
    } else {
        open_inventory_ui(
            &mut commands,
            &mut images,
            assets.as_deref_mut(),
            &catalog,
            &inventory,
            &mut open,
            &mut selected,
            &selected_tab,
            &mut focus,
            scale.0,
            item_target.as_deref(),
            &mut icons,
        );
    }
}

/// Press log for the BAG button: the inventory opened or closed
/// this frame because of a BAG press. Runs after
/// `toggle_inventory` + `apply_inventory_requests` in the Update
/// tuple, so it sees the transition in the frame it happens.
/// (The toggle is at Bevy's 16-parameter limit, so the outcome
/// is observed here instead of logged there — a keyboard B press
/// or a cross-menu request finds no pending BAG press and logs
/// nothing; a BAG press blocked by the pause gate never flips
/// the state, so the flush logs it bare.)
fn log_bag_press_outcome(
    open: Res<InventoryOpen>,
    mut press_log: Option<ResMut<crate::touch::PressLog>>,
) {
    if !open.is_changed() {
        return;
    }
    let Some(log) = press_log.as_deref_mut() else {
        return;
    };
    if let Some(count) = log.claim(crate::touch::PressButton::Bag) {
        let outcome = if open.0 {
            "inventory opened"
        } else {
            "inventory closed"
        };
        crate::startup_log::log(&crate::touch::PressLog::format_line(
            crate::touch::PressButton::Bag,
            count,
            Some(outcome),
        ));
    }
}

/// Consume a cross-menu open/close request (UI milestone 2):
/// the character menu's Items tab opens the inventory, the MAP
/// button hands off from the inventory back to the character
/// menu. Runs after `toggle_inventory` in the Update tuple so
/// a same-frame BAG press and request do not double-handle —
/// the request wins the frame it is set.
#[allow(clippy::too_many_arguments)]
fn apply_inventory_requests(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut assets: Option<ResMut<crate::assets::AssetProvider>>,
    catalog: Res<ItemCatalog>,
    inventory: ResMut<Inventory>,
    mut open: ResMut<InventoryOpen>,
    mut selected: ResMut<SelectedItem>,
    selected_tab: Res<SelectedTab>,
    mut focus: ResMut<InventoryFocus>,
    scale: Res<UiScale>,
    item_target: Option<Res<crate::item_model::ItemModelTarget>>,
    mut icons: ResMut<InterfaceIcons>,
    ui_roots: Query<Entity, With<InventoryUiRoot>>,
    item_model_entities: Query<Entity, With<crate::item_model::ItemModelEntity>>,
    mut request: ResMut<InventoryRequest>,
) {
    let Some(action) = request.0.take() else {
        return;
    };
    match action {
        InventoryAction::Open => {
            open_inventory_ui(
                &mut commands,
                &mut images,
                assets.as_deref_mut(),
                &catalog,
                &inventory,
                &mut open,
                &mut selected,
                &selected_tab,
                &mut focus,
                scale.0,
                item_target.as_deref(),
                &mut icons,
            );
        }
        InventoryAction::Close => {
            close_inventory_ui(&mut commands, &ui_roots, &item_model_entities, &mut open);
        }
    }
}

/// Rebuild the inventory UI when anything it shows changed
/// while open: the inventory (a pickup, an equip, a consumed
/// potion, a drop), the tab, the selection (the card + model
/// follow the selection, so a row click or arrow key rebuilds
/// too), or the focus. The whole UI is rebuilt — a few dozen
/// nodes, cheaper than diffing rows — while the item-model rig
/// stays put (it is not part of the UI root). The list scroll
/// is recomputed from the selection at build time (the
/// centered list), so no scroll offset is preserved.
// Bevy systems take their inputs as parameters by design; the
// count is the dependency list, not a smell to refactor away.
#[allow(clippy::too_many_arguments)]
fn refresh_inventory_ui(
    mut commands: Commands,
    inventory: Res<Inventory>,
    catalog: Res<ItemCatalog>,
    open: Res<InventoryOpen>,
    mut selected: ResMut<SelectedItem>,
    selected_tab: Res<SelectedTab>,
    focus: Res<InventoryFocus>,
    scale: Res<UiScale>,
    item_target: Option<Res<crate::item_model::ItemModelTarget>>,
    icons: Res<InterfaceIcons>,
    ui_roots: Query<Entity, With<InventoryUiRoot>>,
) {
    if !open.0 {
        return;
    }
    if !inventory.is_changed()
        && !selected_tab.is_changed()
        && !selected.is_changed()
        && !focus.is_changed()
    {
        return;
    }
    // The selection must be visible under the current tab: a
    // consumed/dropped selected item, or a tab switch away from
    // it, moves the selection to the first visible item (or
    // none, when the tab is empty).
    let visible = visible_stacks(&inventory, &catalog, selected_tab.0);
    let selection_visible = selected
        .0
        .map(|base| visible.iter().any(|s| s.base_form_id == base))
        .unwrap_or(false);
    if !selection_visible {
        selected.0 = visible.first().map(|s| s.base_form_id);
    }
    for entity in ui_roots.iter() {
        commands.entity(entity).despawn();
    }
    spawn_inventory_ui(
        &mut commands,
        &catalog,
        &inventory,
        &selected,
        &selected_tab,
        &focus,
        scale.0,
        item_target.as_deref(),
        &icons,
    );
}

/// The primary action on the selected item, the game's E key:
/// equip a weapon/armor piece, eat food, drink a potion. Books,
/// keys, and clutter have no E action yet (reading is a later
/// milestone). Returns true when something happened.
fn primary_action(
    inventory: &mut Inventory,
    catalog: &ItemCatalog,
    selected: &SelectedItem,
    dirty: &mut DollDirty,
) -> bool {
    let Some(base) = selected.0 else {
        return false;
    };
    let Some(def) = catalog.def(base) else {
        return false;
    };
    match def.kind {
        ItemKind::Weapon | ItemKind::Armor => match inventory.equip(base, def) {
            EquipOutcome::Equipped => {
                dirty.0 = true;
                crate::startup_log::log(&format!("equipped {}", def.name));
                true
            }
            EquipOutcome::Unequipped => {
                dirty.0 = true;
                crate::startup_log::log(&format!("unequipped {}", def.name));
                true
            }
            EquipOutcome::NotEquippable => false,
        },
        ItemKind::Food | ItemKind::Potion => {
            let consumed = inventory.use_item(base, def) == UseOutcome::Consumed;
            if consumed {
                let verb = if def.kind == ItemKind::Food {
                    "ate"
                } else {
                    "drank"
                };
                crate::startup_log::log(&format!("{verb} {}", def.name));
            }
            consumed
        }
        _ => false,
    }
}

/// Lowest vertex Y of one converted mesh (None when it carries
/// no positions). The drop origin is raised by `-min_y` when
/// negative, so a centre-origin model rests on the floor instead
/// of burying its lower half (Nordic Barnacle: min Y -0.108 m,
/// half its 0.217 m height).
fn mesh_min_y(mesh: &Mesh) -> Option<f32> {
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return None;
    };
    positions.iter().map(|v| v[1]).reduce(f32::min)
}

/// Drop one of the selected item at the player's feet, as a real
/// world object that can be picked up again: the ground model
/// spawns as scene entities at the drop position, and the item
/// registers in the pickup index under a synthetic ref ID, so
/// the existing pickup system (M17) handles the take-back with
/// no changes. The drop origin is raised by the model's
/// below-origin extent, so the model RESTS on the floor (its
/// origin is often its centre — dropped at feet level, a
/// barnacle buried half its height). An equipped item unequips
/// first. Returns false — and removes nothing — when there is
/// no selected item, no player, no asset provider, or no ground
/// model for the item.
#[allow(clippy::too_many_arguments)]
pub fn drop_item(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    assets: &mut crate::assets::AssetProvider,
    catalog: &ItemCatalog,
    inventory: &mut Inventory,
    selected: &SelectedItem,
    pickupables: &mut crate::interact::Pickupables,
    dropped_ids: &mut DroppedRefIds,
    dirty: &mut DollDirty,
    player_transform: &Transform,
) -> bool {
    let Some(base) = selected.0 else {
        return false;
    };
    let Some(def) = catalog.def(base) else {
        return false;
    };
    let Some(ground_model) = &def.ground_model else {
        crate::startup_log::log(&format!("cannot drop {}: no ground model", def.name));
        return false;
    };
    // The drop position: at the player's feet, a step ahead, on
    // the floor. The player transform is the camera (eye level),
    // so the feet are EYE_HEIGHT below it.
    let feet = Vec3::new(
        player_transform.translation.x,
        player_transform.translation.y - crate::player::EYE_HEIGHT,
        player_transform.translation.z,
    );
    let forward = player_transform.forward();
    let flat = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let mut drop_position = feet + flat * 0.8;
    // Load the ground model BEFORE touching the inventory — a
    // missing/unparsable mesh aborts the drop instead of losing
    // the item.
    let nif_path = format!("meshes\\{ground_model}");
    let Some(bytes) = assets.read_file(&nif_path) else {
        crate::startup_log::log(&format!(
            "cannot drop {}: mesh not found: {nif_path}",
            def.name
        ));
        return false;
    };
    let Ok(nif) = dovah_nif::NifFile::from_bytes(&bytes) else {
        crate::startup_log::log(&format!(
            "cannot drop {}: NIF parse failed: {nif_path}",
            def.name
        ));
        return false;
    };
    let sub_meshes = nif.meshes();
    if sub_meshes.is_empty() {
        crate::startup_log::log(&format!("cannot drop {}: mesh has no geometry", def.name));
        return false;
    }
    // Convert the ground model BEFORE touching the inventory:
    // the drop origin is raised by the model's below-origin
    // extent, so the model rests on the floor instead of
    // burying its lower half (see `mesh_min_y`). A model whose
    // geometry already sits on or above its origin (min Y >= 0)
    // drops at feet level, unchanged.
    let mut converted: Vec<(Mesh, Option<String>)> = Vec::new();
    let mut min_y: Option<f32> = None;
    for sub in &sub_meshes {
        let Some(mesh) = crate::convert::nif_mesh_to_bevy(sub) else {
            continue;
        };
        if let Some(y) = mesh_min_y(&mesh) {
            min_y = Some(min_y.map_or(y, |m| m.min(y)));
        }
        converted.push((mesh, sub.textures.diffuse.clone()));
    }
    if let Some(y) = min_y {
        if y < 0.0 {
            drop_position.y -= y;
        }
    }
    // An equipped item comes off first; the doll rebuilds from
    // the dirty flag.
    if inventory.is_equipped(base) {
        inventory.equip(base, def);
        dirty.0 = true;
    }
    if !inventory.remove(base, 1) {
        return false;
    }
    // Spawn the ground model as scene entities at the drop
    // position (identity rotation — the pickup despawn matches
    // on the registered position + rotation, and both come from
    // here). The material path is the shared one (player_model):
    // textured when the diffuse resolves, plain grey otherwise.
    let rotation = Quat::IDENTITY;
    let transform = Transform {
        translation: drop_position,
        rotation,
        scale: Vec3::ONE,
    };
    let mut material_cache: HashMap<Option<String>, Handle<StandardMaterial>> = HashMap::new();
    let mut spawned = 0;
    for (mesh, diffuse) in converted {
        let material = crate::player_model::doll_material(
            materials,
            images,
            assets,
            &mut material_cache,
            &diffuse,
        );
        commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            transform,
            crate::scene::SceneEntity,
        ));
        spawned += 1;
    }
    // Register the pickupable under a synthetic ref ID. The
    // counter counts down from 0xFF00_0000; it wraps only after
    // ~16M drops.
    let ref_form_id = dropped_ids.0;
    dropped_ids.0 = dropped_ids.0.wrapping_sub(1);
    pickupables.items.insert(
        ref_form_id,
        crate::interact::Pickupable {
            base_form_id: base,
            position: drop_position,
            rotation,
            harvest_name: None,
        },
    );
    crate::startup_log::log(&format!(
        "dropped {} at ({:.1}, {:.1}, {:.1}) ({spawned} entities, ref 0x{ref_form_id:08x})",
        def.name, drop_position.x, drop_position.y, drop_position.z
    ));
    true
}

/// Keyboard control while the inventory is open, the PC game's
/// keys: Left/Right switch focus between the category and item
/// lists, Up/Down navigate the focused list, E equips/uses
/// (or focuses the items when the categories are focused), Tab
/// and Escape close. Both lists WRAP at the ends (the game's
/// centered lists cycle — a judgment call from how the game
/// behaves, not something the .swf proves). (B is the toggle
/// system's key; the BAG touch button closes too. R is the
/// drop key — it lives in `inventory_drop_key`, split out
/// because one system holding the key handling AND the drop
/// spawner's resources exceeds Bevy's 16-parameter system
/// limit.) No-op while the inventory is closed.
#[allow(clippy::too_many_arguments)]
fn inventory_keys(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    catalog: Res<ItemCatalog>,
    mut inventory: ResMut<Inventory>,
    mut open: ResMut<InventoryOpen>,
    mut selected: ResMut<SelectedItem>,
    mut selected_tab: ResMut<SelectedTab>,
    mut focus: ResMut<InventoryFocus>,
    mut dirty: ResMut<DollDirty>,
    ui_roots: Query<Entity, With<InventoryUiRoot>>,
    item_model_entities: Query<Entity, With<crate::item_model::ItemModelEntity>>,
) {
    if !open.0 {
        return;
    }
    if keyboard.just_pressed(KeyCode::Escape) || keyboard.just_pressed(KeyCode::Tab) {
        close_inventory_ui(&mut commands, &ui_roots, &item_model_entities, &mut open);
        return;
    }
    if keyboard.just_pressed(KeyCode::ArrowLeft) || keyboard.just_pressed(KeyCode::ArrowRight) {
        *focus = match *focus {
            InventoryFocus::Categories => InventoryFocus::Items,
            InventoryFocus::Items => InventoryFocus::Categories,
        };
        // Focusing the items with no selection picks the first
        // visible item (the card is never empty when the tab
        // has items).
        if *focus == InventoryFocus::Items && selected.0.is_none() {
            selected.0 = visible_stacks(&inventory, &catalog, selected_tab.0)
                .first()
                .map(|s| s.base_form_id);
        }
    }
    if keyboard.just_pressed(KeyCode::KeyE) {
        match *focus {
            InventoryFocus::Categories => {
                *focus = InventoryFocus::Items;
                if selected.0.is_none() {
                    selected.0 = visible_stacks(&inventory, &catalog, selected_tab.0)
                        .first()
                        .map(|s| s.base_form_id);
                }
            }
            InventoryFocus::Items => {
                primary_action(&mut inventory, &catalog, &selected, &mut dirty);
            }
        }
    }
    if keyboard.just_pressed(KeyCode::ArrowUp) || keyboard.just_pressed(KeyCode::ArrowDown) {
        let up = keyboard.just_pressed(KeyCode::ArrowUp);
        match *focus {
            InventoryFocus::Categories => {
                let current = InventoryTab::ALL
                    .iter()
                    .position(|t| *t == selected_tab.0)
                    .unwrap_or(0);
                let len = InventoryTab::ALL.len();
                let next = if up {
                    (current + len - 1) % len
                } else {
                    (current + 1) % len
                };
                selected_tab.0 = InventoryTab::ALL[next];
                selected.0 = visible_stacks(&inventory, &catalog, selected_tab.0)
                    .first()
                    .map(|s| s.base_form_id);
            }
            InventoryFocus::Items => {
                let visible = visible_stacks(&inventory, &catalog, selected_tab.0);
                if visible.is_empty() {
                    return;
                }
                let current = selected
                    .0
                    .and_then(|base| visible.iter().position(|s| s.base_form_id == base));
                let next = match (up, current) {
                    (true, Some(i)) => (i + visible.len() - 1) % visible.len(),
                    (true, None) => 0,
                    (false, Some(i)) => (i + 1) % visible.len(),
                    (false, None) => 0,
                };
                selected.0 = Some(visible[next].base_form_id);
            }
        }
    }
}

/// The drop key (R) while the inventory is open: drops one of
/// the selected item at the player's feet. Split from
/// `inventory_keys` for the 16-parameter system limit (see
/// above). No-op while the inventory is closed, and a no-op
/// when the drop cannot spawn (no provider, no player, no
/// ground model) — `drop_item` refuses instead of losing the
/// item.
#[allow(clippy::too_many_arguments)]
fn inventory_drop_key(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut assets: Option<ResMut<crate::assets::AssetProvider>>,
    catalog: Res<ItemCatalog>,
    mut inventory: ResMut<Inventory>,
    open: Res<InventoryOpen>,
    selected: Res<SelectedItem>,
    mut pickupables: Option<ResMut<crate::interact::Pickupables>>,
    mut dropped_ids: Option<ResMut<DroppedRefIds>>,
    mut dirty: ResMut<DollDirty>,
    player: Query<&Transform, With<crate::scene::Player>>,
) {
    if !open.0 || !keyboard.just_pressed(KeyCode::KeyR) {
        return;
    }
    let (Some(assets), Some(pickupables), Some(dropped_ids)) = (
        assets.as_deref_mut(),
        pickupables.as_deref_mut(),
        dropped_ids.as_deref_mut(),
    ) else {
        return;
    };
    let Ok(player_transform) = player.single() else {
        return;
    };
    let player_transform = *player_transform;
    // The destructured bindings are already `&mut` (from
    // `as_deref_mut`) — pass them straight through.
    drop_item(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        assets,
        &catalog,
        &mut inventory,
        &selected,
        pickupables,
        dropped_ids,
        &mut dirty,
        &player_transform,
    );
}

/// Respawn the doll when an equip/unequip/drop dirtied it
/// (the flag is consumed here — one rebuild per frame at most,
/// however many changes stacked up). The parts are despawned
/// and respawned; the rig (camera + light + target) stays.
/// No-op while the inventory is closed (nothing to show), when
/// the flag is clear, or when no doll rig exists — the
/// inventory no longer shows the doll, so the flag sits set
/// until a screen that shows the doll consumes it (the flag is
/// NOT cleared in that case: the doll is still dirty).
#[allow(clippy::too_many_arguments)]
fn rebuild_doll_if_dirty(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut assets: Option<ResMut<crate::assets::AssetProvider>>,
    catalog: Res<ItemCatalog>,
    inventory: Res<Inventory>,
    open: Res<InventoryOpen>,
    mut dirty: ResMut<DollDirty>,
    doll_parts: Query<Entity, With<crate::player_model::DollPart>>,
    doll_entities: Query<Entity, With<crate::player_model::DollEntity>>,
) {
    if !open.0 || !dirty.0 || doll_entities.is_empty() {
        return;
    }
    dirty.0 = false;
    let Some(assets) = assets.as_deref_mut() else {
        return;
    };
    for entity in doll_parts.iter() {
        commands.entity(entity).despawn();
    }
    crate::player_model::spawn_doll_parts(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        assets,
        &catalog,
        &inventory,
    );
}

/// The gold the game uses for highlights (the selected
/// category, the equipped marker).
const GOLD: Color = Color::srgb(0.85, 0.7, 0.35);
/// The list/card/bar background: the .swf's black fill
/// (0,0,0,153) — 60% opaque.
const PANEL_BG: Color = Color::srgba(0.0, 0.0, 0.0, 0.6);
/// The panel border: the .swf's grey fill (153,153,153,153).
const PANEL_BORDER: Color = Color::srgba(0.6, 0.6, 0.6, 0.6);
/// The category row divider: the .swf's fill (215,215,215,92).
const DIVIDER: Color = Color::srgba(0.84, 0.84, 0.84, 0.36);
/// Unselected row text (white, dimmed — the selected row reads
/// full white).
const TEXT_DIM: Color = Color::srgba(1.0, 1.0, 1.0, 0.7);
/// The card/bottom-bar label grey (#999999, the .swf's label
/// color).
const LABEL_GREY: Color = Color::srgb(0.6, 0.6, 0.6);

/// The vanilla menu stage (the .swf stage): 1280x720. Every
/// layout constant below is a stage pixel; `px()` scales it by
/// `UiScale` at build time.
pub const STAGE_WIDTH: f32 = 1280.0;
pub const STAGE_HEIGHT: f32 = 720.0;
/// Category list: x=20, w=210 (inventorylists.swf).
pub const CATEGORY_LIST_X: f32 = 20.0;
pub const CATEGORY_LIST_W: f32 = 210.0;
/// The "CATEGORY" label at (15,22) size 22; the list starts at
/// y=60.
pub const CATEGORY_LABEL_X: f32 = 15.0;
pub const CATEGORY_LABEL_Y: f32 = 22.0;
pub const CATEGORY_LIST_TOP: f32 = 60.0;
/// Category rows: 40 px pitch; the selected row is 80 px tall
/// (Entry7 at y=280, Entry8 at y=360).
pub const CATEGORY_ROW_H: f32 = 40.0;
pub const CATEGORY_ROW_SELECTED_H: f32 = 80.0;
/// Items list: x=230, w=270 (inventorylists.swf); the list
/// starts at y=28.
pub const ITEM_LIST_X: f32 = 230.0;
pub const ITEM_LIST_W: f32 = 270.0;
pub const ITEM_LIST_TOP: f32 = 28.0;
/// Item rows: 35 px pitch; the selected row is 65 px tall
/// (Entry9 at y=315, Entry10 at y=380). The selected row sits
/// at a fixed y=315 when the list scrolls (the centered list).
pub const ITEM_ROW_H: f32 = 35.0;
pub const ITEM_ROW_SELECTED_H: f32 = 65.0;
pub const ITEM_SELECTED_Y: f32 = 315.0;
/// Item model viewport: x=532 (aligned with the card), y=8,
/// 740x410 — derived from the card + catcher positions; the
/// .swf has no exact viewport rect.
pub const MODEL_VIEW_X: f32 = 532.0;
pub const MODEL_VIEW_Y: f32 = 8.0;
pub const MODEL_VIEW_W: f32 = 740.0;
pub const MODEL_VIEW_H: f32 = 410.0;
/// Item card: holder at (532, 434.3), 428 wide; slim 153 /
/// wide 185 tall (itemcard.swf).
pub const CARD_X: f32 = 532.0;
pub const CARD_Y: f32 = 434.3;
pub const CARD_W: f32 = 428.0;
pub const CARD_H_SLIM: f32 = 153.0;
pub const CARD_H_WIDE: f32 = 185.0;
/// Bottom bar: y=645, 75 tall (bottombar.swf).
pub const BOTTOM_BAR_Y: f32 = 645.0;
pub const BOTTOM_BAR_H: f32 = 75.0;

/// Stage px -> scaled px.
pub(crate) fn px(scale: f32, stage_px: f32) -> Val {
    Val::Px(stage_px * scale)
}

/// Stage px -> scaled font size.
pub(crate) fn fs(scale: f32, stage_px: f32) -> FontSize {
    FontSize::Px(stage_px * scale)
}

/// The centered list's scroll offset: the selected row's top
/// sits at `center_y` when the list can scroll that far,
/// clamped to the list's ends. `selected_top` is the selected
/// row's offset in the content, `content_h` the content height,
/// `viewport_h` the visible height. Pure — unit-tested.
pub(crate) fn centered_scroll_y(
    selected_top: f32,
    center_y: f32,
    content_h: f32,
    viewport_h: f32,
) -> f32 {
    let max = (content_h - viewport_h).max(0.0);
    (selected_top - center_y).clamp(0.0, max)
}

/// Load the game's key icons from Interface.bsa (see
/// `InterfaceIcons`). Each icon is optional — a missing file
/// (or an undecodable DDS) leaves that button text-only.
fn load_interface_icons(
    images: &mut Assets<Image>,
    assets: &mut crate::assets::AssetProvider,
) -> InterfaceIcons {
    let mut load = |name: &str| -> Option<Handle<Image>> {
        let path = format!("interface/exported/{name}.png.dds");
        let bytes = assets.read_file(&path)?;
        let image = crate::convert::dds_to_bevy_image(&bytes)?;
        Some(images.add(image))
    };
    InterfaceIcons {
        e: load("e"),
        r: load("r"),
        tab: load("tab"),
        loaded: true,
    }
}

/// Load the key icons when a dialogue is open and the inventory
/// never loaded them: the dialogue's Goodbye button shows the
/// Tab icon, and `open_dialogue` takes no images/assets. The
/// inventory loads them synchronously at open; this is only the
/// dialogue-first path. `refresh_dialogue_ui` rebuilds on the
/// icons' change, so the button picks the icon up a frame
/// later.
fn ensure_interface_icons(
    mut images: ResMut<Assets<Image>>,
    mut assets: Option<ResMut<crate::assets::AssetProvider>>,
    dialogue_open: Option<Res<crate::dialogue::DialogueOpen>>,
    pause_open: Option<Res<crate::pause::PauseOpen>>,
    mut icons: ResMut<InterfaceIcons>,
) {
    // The icons load the first time a menu that needs them
    // opens — dialogue (M18) or the pause screen (UI milestone
    // 1, whose bottom bar shows the [Tab] Close icon).
    let needed =
        dialogue_open.map(|o| o.0).unwrap_or(false) || pause_open.map(|o| o.0).unwrap_or(false);
    if icons.loaded || !needed {
        return;
    }
    if let Some(assets) = assets.as_deref_mut() {
        *icons = load_interface_icons(&mut images, assets);
    }
}

/// Spawn the inventory UI — the game's own layout, from the
/// .swf dumps (see the module doc): the category + item lists
/// on the left, the selected item's 3D model on the right, the
/// item card below it, the bottom bar at y=645. Positions are
/// stage pixels scaled by `scale`; the stage is centered in
/// the window (full-screen at 1280x720). The root has NO
/// background — the world shows through, as in the game; only
/// the lists, the card, and the bottom bar carry panels. The
/// list scroll is the centered offset, computed from the
/// selection (the selected row sits at ITEM_SELECTED_Y when
/// the list can scroll that far).
#[allow(clippy::too_many_arguments)]
fn spawn_inventory_ui(
    commands: &mut Commands,
    catalog: &ItemCatalog,
    inventory: &Inventory,
    selected: &SelectedItem,
    selected_tab: &SelectedTab,
    focus: &InventoryFocus,
    scale: f32,
    item_target: Option<&crate::item_model::ItemModelTarget>,
    icons: &InterfaceIcons,
) {
    let _ = focus; // The focus drives the keys, not the look:
                   // the game shows no focus highlight of its own.
    let weight = inventory.total_weight(catalog);
    let visible = visible_stacks(inventory, catalog, selected_tab.0);
    let selected_def = selected.0.and_then(|b| catalog.def(b));
    let selected_index = selected
        .0
        .and_then(|b| visible.iter().position(|s| s.base_form_id == b));
    // The bottom bar's totals, as the game shows them: the
    // equipped weapon's damage and the summed armor rating.
    let damage = inventory
        .equipped_weapon
        .and_then(|b| catalog.def(b))
        .map(|d| d.damage)
        .unwrap_or(0);
    let armor: f32 = inventory
        .equipped_armor
        .values()
        .filter_map(|b| catalog.def(*b))
        .map(|d| d.armor_rating)
        .sum();
    let primary_label = selected_def.and_then(primary_action_label);
    // The centered scroll (screen px — the stage math times
    // the scale): the selected row's content offset is its
    // index times the 35 px pitch (every row above it is the
    // short kind); the content is one selected row taller.
    let content_h = visible.len() as f32 * ITEM_ROW_H
        + if selected.0.is_some() {
            ITEM_ROW_SELECTED_H - ITEM_ROW_H
        } else {
            0.0
        };
    let viewport_h = BOTTOM_BAR_Y - ITEM_LIST_TOP;
    let scroll_y = match selected_index {
        Some(i) => {
            centered_scroll_y(
                i as f32 * ITEM_ROW_H,
                ITEM_SELECTED_Y,
                content_h,
                viewport_h,
            ) * scale
        }
        None => 0.0,
    };
    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            InventoryUiRoot,
        ))
        .with_children(|root| {
            root.spawn(Node {
                width: px(scale, STAGE_WIDTH),
                height: px(scale, STAGE_HEIGHT),
                ..Default::default()
            })
            .with_children(|stage| {
                // The category list: panel + "CATEGORY" label +
                // ten rows. The panel runs the full height
                // (the .swf background is 754 tall — it runs
                // under the bottom bar, which draws over it).
                stage
                    .spawn((
                        Node {
                            width: px(scale, CATEGORY_LIST_W),
                            height: px(scale, STAGE_HEIGHT),
                            position_type: PositionType::Absolute,
                            left: px(scale, CATEGORY_LIST_X),
                            top: Val::Px(0.0),
                            border: bevy::ui::UiRect::right(px(scale, 1.0)),
                            ..Default::default()
                        },
                        BackgroundColor(PANEL_BG),
                        BorderColor::all(PANEL_BORDER),
                    ))
                    .with_children(|panel| {
                        panel.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: px(scale, CATEGORY_LABEL_X),
                                top: px(scale, CATEGORY_LABEL_Y),
                                ..Default::default()
                            },
                            Text::new("CATEGORY"),
                            TextFont {
                                font_size: fs(scale, 22.0),
                                ..Default::default()
                            },
                            TextColor(Color::WHITE),
                        ));
                        panel
                            .spawn(Node {
                                position_type: PositionType::Absolute,
                                left: px(scale, CATEGORY_LABEL_X),
                                top: px(scale, CATEGORY_LIST_TOP),
                                flex_direction: FlexDirection::Column,
                                ..Default::default()
                            })
                            .with_children(|list| {
                                for tab in InventoryTab::ALL {
                                    let is_selected = selected_tab.0 == tab;
                                    let row_h = if is_selected {
                                        CATEGORY_ROW_SELECTED_H
                                    } else {
                                        CATEGORY_ROW_H
                                    };
                                    list.spawn((
                                        Node {
                                            width: px(scale, 192.0),
                                            height: px(scale, row_h),
                                            flex_shrink: 0.0,
                                            ..Default::default()
                                        },
                                        InventoryCategoryRow { tab },
                                    ))
                                    .observe(on_category_click)
                                    .with_children(|row| {
                                        // The divider under the
                                        // name (unselected rows
                                        // only — the selected
                                        // entry has none): a
                                        // faint line at y=20,
                                        // behind the text, so
                                        // it shows to the right
                                        // of the name.
                                        if !is_selected {
                                            row.spawn((
                                                Node {
                                                    position_type: PositionType::Absolute,
                                                    left: Val::Px(0.0),
                                                    top: px(scale, 20.0),
                                                    width: px(scale, 153.0),
                                                    height: px(scale, 2.0),
                                                    ..Default::default()
                                                },
                                                BackgroundColor(DIVIDER),
                                            ));
                                        }
                                        row.spawn((
                                            Node {
                                                position_type: PositionType::Absolute,
                                                left: px(scale, 2.0),
                                                top: px(
                                                    scale,
                                                    if is_selected { 24.6 } else { 8.2 },
                                                ),
                                                ..Default::default()
                                            },
                                            Text::new(tab.label()),
                                            TextFont {
                                                font_size: fs(
                                                    scale,
                                                    if is_selected { 26.0 } else { 20.0 },
                                                ),
                                                ..Default::default()
                                            },
                                            TextColor(if is_selected { GOLD } else { TEXT_DIM }),
                                            // No wrap: a wrapped
                                            // name would spill
                                            // onto the next row
                                            // (the default font
                                            // is wider than the
                                            // game's condensed
                                            // one).
                                            TextLayout::no_wrap(),
                                        ));
                                    });
                                }
                            });
                    });
                // The items list: panel + the scrollable
                // centered list. The list area is 252 wide at
                // (5, 28), running to the bottom bar.
                stage
                    .spawn((
                        Node {
                            width: px(scale, ITEM_LIST_W),
                            height: px(scale, STAGE_HEIGHT),
                            position_type: PositionType::Absolute,
                            left: px(scale, ITEM_LIST_X),
                            top: Val::Px(0.0),
                            border: bevy::ui::UiRect::right(px(scale, 1.0)),
                            ..Default::default()
                        },
                        BackgroundColor(PANEL_BG),
                        BorderColor::all(PANEL_BORDER),
                    ))
                    .with_children(|panel| {
                        let mut list = panel.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: px(scale, 5.0),
                                top: px(scale, ITEM_LIST_TOP),
                                width: px(scale, 252.0),
                                height: px(scale, viewport_h),
                                flex_direction: FlexDirection::Column,
                                overflow: Overflow::scroll_y(),
                                ..Default::default()
                            },
                            InventoryList,
                            ScrollPosition(Vec2::new(0.0, scroll_y)),
                        ));
                        list.with_children(|list| {
                            for stack in &visible {
                                let Some(def) = catalog.def(stack.base_form_id) else {
                                    continue;
                                };
                                let is_selected = selected.0 == Some(stack.base_form_id);
                                let equipped = inventory.is_equipped(stack.base_form_id);
                                let row_h = if is_selected {
                                    ITEM_ROW_SELECTED_H
                                } else {
                                    ITEM_ROW_H
                                };
                                list.spawn((
                                    Node {
                                        width: Val::Percent(100.0),
                                        height: px(scale, row_h),
                                        flex_shrink: 0.0,
                                        ..Default::default()
                                    },
                                    InventoryRow {
                                        base_form_id: stack.base_form_id,
                                    },
                                ))
                                .observe(on_row_click)
                                .with_children(|row| {
                                    // The equipped marker (">"
                                    // — the default font has
                                    // no triangle glyph) in
                                    // the 20 px slot at the
                                    // row's left.
                                    row.spawn((
                                        Node {
                                            position_type: PositionType::Absolute,
                                            left: Val::Px(0.0),
                                            top: px(scale, if is_selected { 24.6 } else { 9.7 }),
                                            width: px(scale, 20.0),
                                            ..Default::default()
                                        },
                                        Text::new(if equipped { ">" } else { "" }),
                                        TextFont {
                                            font_size: fs(scale, 20.0),
                                            ..Default::default()
                                        },
                                        TextColor(GOLD),
                                    ));
                                    // The game's count format:
                                    // "Iron Arrow (25)".
                                    let count = if stack.count > 1 {
                                        format!(" ({})", stack.count)
                                    } else {
                                        String::new()
                                    };
                                    row.spawn((
                                        Node {
                                            position_type: PositionType::Absolute,
                                            left: px(scale, 22.0),
                                            top: px(scale, if is_selected { 20.6 } else { 5.7 }),
                                            ..Default::default()
                                        },
                                        Text::new(format!("{}{}", def.name, count)),
                                        TextFont {
                                            font_size: fs(scale, 20.0),
                                            ..Default::default()
                                        },
                                        TextColor(if is_selected {
                                            Color::WHITE
                                        } else {
                                            TEXT_DIM
                                        }),
                                        // No wrap: a long name
                                        // ("Potion of Minor
                                        // Healing") wraps onto
                                        // the next row without
                                        // this — the default
                                        // font is wider than
                                        // the game's condensed
                                        // one, so the name may
                                        // run a few px past
                                        // the list edge
                                        // instead.
                                        TextLayout::no_wrap(),
                                    ));
                                });
                            }
                        });
                    });
                // The selected item's 3D model (item_model.rs):
                // the render target, draggable to rotate.
                // Absent when there is no target (the headless
                // test app) — the card still shows.
                if let Some(target) = item_target {
                    stage
                        .spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: px(scale, MODEL_VIEW_X),
                                top: px(scale, MODEL_VIEW_Y),
                                width: px(scale, MODEL_VIEW_W),
                                height: px(scale, MODEL_VIEW_H),
                                ..Default::default()
                            },
                            ImageNode::new(target.image.clone()),
                        ))
                        .observe(crate::item_model::on_item_model_drag);
                }
                // The item card: type line, name, stats, and
                // the effects/description block on wide cards.
                // Nothing shows when nothing is selected (an
                // empty tab) — the game shows no card either.
                if let Some(def) = selected_def {
                    let wide = card_is_wide(def);
                    let card_h = if wide { CARD_H_WIDE } else { CARD_H_SLIM };
                    stage
                        .spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: px(scale, CARD_X),
                                top: px(scale, CARD_Y),
                                width: px(scale, CARD_W),
                                height: px(scale, card_h),
                                border: bevy::ui::UiRect::all(px(scale, 1.0)),
                                ..Default::default()
                            },
                            BackgroundColor(PANEL_BG),
                            BorderColor::all(PANEL_BORDER),
                        ))
                        .with_children(|card| {
                            // The type line (the .swf's skill
                            // line slot — see type_label).
                            card.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(scale, 31.0),
                                    top: px(scale, 10.5),
                                    ..Default::default()
                                },
                                Text::new(def.kind.type_label()),
                                TextFont {
                                    font_size: fs(scale, 16.0),
                                    ..Default::default()
                                },
                                TextColor(LABEL_GREY),
                            ));
                            card.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(scale, 20.3),
                                    top: px(scale, 36.3),
                                    width: px(scale, 392.0),
                                    ..Default::default()
                                },
                                Text::new(def.name.clone()),
                                TextFont {
                                    font_size: fs(scale, 30.0),
                                    ..Default::default()
                                },
                                TextColor(Color::WHITE),
                                // No wrap: a wrapped name
                                // would run onto the stats
                                // row (a long name may run
                                // past the card edge instead
                                // — the default font is wider
                                // than the game's condensed
                                // one).
                                TextLayout::no_wrap(),
                            ));
                            // The stats row: one label + value
                            // per stat, the label right-aligned
                            // in the field ending at its value.
                            // (label, value, label_x, value_x)
                            let stats: Vec<(&str, String, f32, f32)> = match def.kind {
                                ItemKind::Weapon => vec![
                                    ("DAMAGE", format!("{}", def.damage), 16.1, 88.0),
                                    ("WEIGHT", format!("{:.1}", def.weight), 146.5, 208.8),
                                    ("VALUE", format!("{}", def.value), 266.8, 323.1),
                                ],
                                ItemKind::Armor => vec![
                                    ("ARMOR", format!("{}", def.armor_rating as i32), 20.6, 88.0),
                                    ("WEIGHT", format!("{:.1}", def.weight), 140.9, 208.8),
                                    ("VALUE", format!("{}", def.value), 266.8, 323.1),
                                ],
                                _ => vec![
                                    ("WEIGHT", format!("{:.1}", def.weight), 63.9, 140.0),
                                    ("VALUE", format!("{}", def.value), 198.9, 254.5),
                                ],
                            };
                            for (label, value, label_x, value_x) in stats {
                                card.spawn((
                                    Node {
                                        position_type: PositionType::Absolute,
                                        left: px(scale, label_x),
                                        top: px(scale, 84.9),
                                        width: px(scale, value_x - label_x),
                                        ..Default::default()
                                    },
                                    Text::new(label),
                                    TextFont {
                                        font_size: fs(scale, 16.0),
                                        ..Default::default()
                                    },
                                    TextColor(LABEL_GREY),
                                    TextLayout::justify(Justify::Right),
                                ));
                                card.spawn((
                                    Node {
                                        position_type: PositionType::Absolute,
                                        left: px(scale, value_x),
                                        top: px(scale, 77.2),
                                        ..Default::default()
                                    },
                                    Text::new(value),
                                    TextFont {
                                        font_size: fs(scale, 30.0),
                                        ..Default::default()
                                    },
                                    TextColor(Color::WHITE),
                                ));
                            }
                            // The effects/description block
                            // (wide cards only).
                            match def.kind {
                                ItemKind::Ingredient => {
                                    // The 2x2 grid: x=11/214,
                                    // y=119.5/147.7, size 18.
                                    for (i, effect) in def.effects.iter().take(4).enumerate() {
                                        let x = if i % 2 == 0 { 11.0 } else { 214.0 };
                                        let y = if i < 2 { 119.5 } else { 147.7 };
                                        card.spawn((
                                            Node {
                                                position_type: PositionType::Absolute,
                                                left: px(scale, x),
                                                top: px(scale, y),
                                                width: px(scale, 200.0),
                                                ..Default::default()
                                            },
                                            Text::new(effect.clone()),
                                            TextFont {
                                                font_size: fs(scale, 18.0),
                                                ..Default::default()
                                            },
                                            TextColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
                                            TextLayout::no_wrap(),
                                        ));
                                    }
                                }
                                ItemKind::Book => {
                                    if let Some(description) = &def.description {
                                        card.spawn((
                                            Node {
                                                position_type: PositionType::Absolute,
                                                left: px(scale, 31.5),
                                                top: px(scale, 121.7),
                                                width: px(scale, 369.0),
                                                ..Default::default()
                                            },
                                            Text::new(description.clone()),
                                            TextFont {
                                                font_size: fs(scale, 20.0),
                                                ..Default::default()
                                            },
                                            TextColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
                                        ));
                                    }
                                }
                                _ => {
                                    // Potion/food effects: one
                                    // line each at (11, 111),
                                    // size 20 — the field
                                    // fits two lines.
                                    for (i, effect) in def.effects.iter().take(2).enumerate() {
                                        card.spawn((
                                            Node {
                                                position_type: PositionType::Absolute,
                                                left: px(scale, 11.0),
                                                top: px(scale, 111.0 + i as f32 * 26.0),
                                                width: px(scale, 407.0),
                                                ..Default::default()
                                            },
                                            Text::new(effect.clone()),
                                            TextFont {
                                                font_size: fs(scale, 20.0),
                                                ..Default::default()
                                            },
                                            TextColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
                                            TextLayout::no_wrap(),
                                        ));
                                    }
                                }
                            }
                        });
                }
                // The bottom bar: buttons left, player info
                // right. Spawned last so it draws over the
                // list panels (the .swf's BottomBar_mc is
                // above InventoryLists_mc in depth).
                stage
                    .spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(0.0),
                            top: px(scale, BOTTOM_BAR_Y),
                            width: px(scale, STAGE_WIDTH),
                            height: px(scale, BOTTOM_BAR_H),
                            border: bevy::ui::UiRect::top(px(scale, 1.0)),
                            ..Default::default()
                        },
                        BackgroundColor(PANEL_BG),
                        BorderColor::all(PANEL_BORDER),
                    ))
                    .with_children(|bar| {
                        // The buttons: Equip / Drop / Close
                        // at x=78/168/258, y=15 (bottombar.swf's
                        // Button0-2 slots). Fixed slots: a
                        // hidden button leaves its slot empty,
                        // it does not shift the others. No key
                        // icons — there are no keys on a phone
                        // (v0.4.13); the word is the tap target.
                        let buttons = [
                            BarButtonSpec {
                                action: BarAction::Primary,
                                label: primary_label,
                                x: 78.0,
                            },
                            BarButtonSpec {
                                action: BarAction::Drop,
                                label: selected_def.map(|_| "Drop"),
                                x: 168.0,
                            },
                            BarButtonSpec {
                                action: BarAction::Close,
                                label: Some("Close"),
                                x: 258.0,
                            },
                        ];
                        for spec in buttons {
                            let Some(label) = spec.label else {
                                continue;
                            };
                            let mut button = bar.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(scale, spec.x),
                                    top: px(scale, 15.0),
                                    flex_direction: FlexDirection::Row,
                                    align_items: AlignItems::Center,
                                    column_gap: px(scale, 4.0),
                                    ..Default::default()
                                },
                                InventoryBarButton {
                                    action: spec.action,
                                },
                            ));
                            // Two observers (the 16-parameter
                            // system limit): the action
                            // buttons carry the drop spawner,
                            // the close button carries the
                            // despawn queries.
                            match spec.action {
                                BarAction::Close => {
                                    button.observe(on_bar_close_click);
                                }
                                _ => {
                                    button.observe(on_bar_action_click);
                                }
                            }
                            // No key prompt: the .swf's buttons
                            // carry a key icon ([E]/[R]/[Tab]),
                            // but there are no keys on a phone
                            // — the word is the tap target
                            // (v0.4.13: "E Equip / R Drop / Tab
                            // Close" on touch). The buttons keep
                            // their click observers.
                            button.with_children(|button| {
                                button.spawn((
                                    Text::new(label),
                                    TextFont {
                                        font_size: fs(scale, 20.0),
                                        ..Default::default()
                                    },
                                    TextColor(Color::WHITE),
                                ));
                            });
                        }
                        // The player info (bottombar.swf's
                        // PlayerInfoCard at (670, 16)):
                        // Damage / Armor / Carry Weight /
                        // Gold. The .swf's field rects are
                        // sized for the game's condensed
                        // font — with the wider default
                        // font, "Carry Weight" wraps and
                        // the carry value collides with the
                        // Gold label, so the pairs are
                        // re-spaced into fixed slots (label
                        // left-aligned, value after it)
                        // instead of the .swf's overlapping
                        // fields. The order and the leftmost
                        // anchor (x=631) follow the .swf.
                        // Carry weight shows as "77/300"
                        // (no decimal, no spaces) — the
                        // game's format.
                        let info: [(&str, String, f32, f32); 4] = [
                            ("Damage", format!("{damage}"), 631.0, 707.0),
                            ("Armor", format!("{}", armor as i32), 780.0, 846.0),
                            (
                                "Carry Weight",
                                format!("{weight:.0}/{CARRY_CAPACITY:.0}"),
                                920.0,
                                1044.0,
                            ),
                            ("Gold", format!("{}", inventory.gold), 1144.0, 1195.0),
                        ];
                        for (label, value, label_x, value_x) in info {
                            bar.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(scale, label_x),
                                    top: px(scale, 16.0 + 5.0),
                                    ..Default::default()
                                },
                                Text::new(label),
                                TextFont {
                                    font_size: fs(scale, 16.0),
                                    ..Default::default()
                                },
                                TextColor(LABEL_GREY),
                                TextLayout::no_wrap(),
                            ));
                            bar.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(scale, value_x),
                                    top: px(scale, 16.0 + 2.0),
                                    ..Default::default()
                                },
                                Text::new(value),
                                TextFont {
                                    font_size: fs(scale, 20.0),
                                    ..Default::default()
                                },
                                TextColor(Color::WHITE),
                                TextLayout::no_wrap(),
                            ));
                        }
                    });
            });
        });
}

/// Click a category row: switch to it (the selection moves to
/// the new category's first visible item) and focus the items.
fn on_category_click(
    event: On<PointerClick>,
    rows: Query<&InventoryCategoryRow>,
    mut selected_tab: ResMut<SelectedTab>,
    mut selected: ResMut<SelectedItem>,
    mut focus: ResMut<InventoryFocus>,
    inventory: Res<Inventory>,
    catalog: Res<ItemCatalog>,
) {
    let Ok(row) = rows.get(event.event_target()) else {
        return;
    };
    selected_tab.0 = row.tab;
    *focus = InventoryFocus::Items;
    selected.0 = visible_stacks(&inventory, &catalog, row.tab)
        .first()
        .map(|s| s.base_form_id);
}

/// Click a row: select that item — or, when it is already
/// selected, run the primary action on it (equip / eat /
/// drink). The second tap is the phone's E key. Either way the
/// items list takes focus.
fn on_row_click(
    event: On<PointerClick>,
    rows: Query<&InventoryRow>,
    mut selected: ResMut<SelectedItem>,
    mut focus: ResMut<InventoryFocus>,
    mut inventory: ResMut<Inventory>,
    catalog: Res<ItemCatalog>,
    mut dirty: ResMut<DollDirty>,
) {
    let Ok(row) = rows.get(event.event_target()) else {
        return;
    };
    *focus = InventoryFocus::Items;
    if selected.0 == Some(row.base_form_id) {
        primary_action(&mut inventory, &catalog, &selected, &mut dirty);
    } else {
        selected.0 = Some(row.base_form_id);
    }
}

/// Click a bottom-bar action button: the primary action (E) or
/// Drop (R), the touch paths for the keyboard keys. (Close has
/// its own observer — the 16-parameter system limit.)
#[allow(clippy::too_many_arguments)]
fn on_bar_action_click(
    event: On<PointerClick>,
    buttons: Query<&InventoryBarButton>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut assets: Option<ResMut<crate::assets::AssetProvider>>,
    catalog: Res<ItemCatalog>,
    mut inventory: ResMut<Inventory>,
    selected: Res<SelectedItem>,
    mut dirty: ResMut<DollDirty>,
    mut pickupables: Option<ResMut<crate::interact::Pickupables>>,
    mut dropped_ids: Option<ResMut<DroppedRefIds>>,
    player: Query<&Transform, With<crate::scene::Player>>,
) {
    let Ok(button) = buttons.get(event.event_target()) else {
        return;
    };
    match button.action {
        BarAction::Primary => {
            primary_action(&mut inventory, &catalog, &selected, &mut dirty);
        }
        BarAction::Drop => {
            let (Some(assets), Some(pickupables), Some(dropped_ids)) = (
                assets.as_deref_mut(),
                pickupables.as_deref_mut(),
                dropped_ids.as_deref_mut(),
            ) else {
                return;
            };
            let Ok(player_transform) = player.single() else {
                return;
            };
            let player_transform = *player_transform;
            // The destructured bindings are already `&mut`
            // (from `as_deref_mut`) — pass them straight
            // through.
            drop_item(
                &mut commands,
                &mut meshes,
                &mut materials,
                &mut images,
                assets,
                &catalog,
                &mut inventory,
                &selected,
                pickupables,
                dropped_ids,
                &mut dirty,
                &player_transform,
            );
        }
        BarAction::Close => {} // This observer never runs on
                               // the close button (it carries on_bar_close_click).
    }
}

/// Click the bottom-bar Close button: close the inventory (the
/// touch path for Tab).
fn on_bar_close_click(
    event: On<PointerClick>,
    buttons: Query<&InventoryBarButton>,
    mut commands: Commands,
    mut open: ResMut<InventoryOpen>,
    ui_roots: Query<Entity, With<InventoryUiRoot>>,
    item_model_entities: Query<Entity, With<crate::item_model::ItemModelEntity>>,
) {
    let Ok(button) = buttons.get(event.event_target()) else {
        return;
    };
    if button.action != BarAction::Close {
        return;
    }
    close_inventory_ui(&mut commands, &ui_roots, &item_model_entities, &mut open);
}

/// The compass-heading helper lives in touch.rs; this module's
/// tests cover the item parsing, the inventory mechanics, and the
/// leveled-list resolver with synthetic records.
#[cfg(test)]
mod tests {
    use super::*;
    use dovah_esm::Field;
    use std::collections::HashSet;

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

    fn record(type_code: &[u8; 4], form_id: u32, fields: Vec<Field>) -> Record {
        Record {
            record_type: *type_code,
            flags: 0,
            form_id,
            version: 44,
            fields,
        }
    }

    fn value_weight(value: i32, weight: f32) -> Vec<u8> {
        [value.to_le_bytes(), weight.to_le_bytes()].concat()
    }

    #[test]
    fn prettify_splits_camel_case_and_digits() {
        assert_eq!(prettify_edid("IronSword"), "Iron Sword");
        assert_eq!(prettify_edid("FoodBread01A"), "Food Bread 01 A");
        assert_eq!(prettify_edid("BasicTankard01"), "Basic Tankard 01");
        assert_eq!(prettify_edid("Gold001"), "Gold 001");
        assert_eq!(prettify_edid("glowDust"), "Glow Dust");
        assert_eq!(prettify_edid("Tankard"), "Tankard");
        assert_eq!(
            prettify_edid("MQ106DragonParchment"),
            "MQ 106 Dragon Parchment"
        );
    }

    #[test]
    fn real_name_from_strings_beats_prettified_edid() {
        // M18: FoodSweetroll's FULL resolves to "Sweetroll" in
        // .strings — the prettified EDID would say "Food
        // Sweetroll". With no strings, the EDID fallback still
        // applies (the M17 behaviour).
        let strings = {
            let mut data = Vec::new();
            let mut dir = Vec::new();
            let entries: [(u32, &str); 1] = [(42, "Sweetroll")];
            for (id, text) in entries {
                dir.push((id, data.len() as u32));
                data.extend_from_slice(text.as_bytes());
                data.push(0);
            }
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
            for (id, offset) in dir {
                bytes.extend_from_slice(&id.to_le_bytes());
                bytes.extend_from_slice(&offset.to_le_bytes());
            }
            bytes.extend_from_slice(&data);
            let general =
                dovah_strings::StringsFile::from_bytes(&bytes, dovah_strings::StringsKind::General)
                    .unwrap();
            crate::dialogue::GameStrings {
                sets: vec![dovah_strings::StringsSet {
                    general,
                    dialogue: dovah_strings::StringsFile::default(),
                    longform: dovah_strings::StringsFile::default(),
                }],
            }
        };
        let rec = record(
            b"ALCH",
            0x0006_5c97,
            vec![
                str_field(b"EDID", "FoodSweetroll"),
                field(b"FULL", 42u32.to_le_bytes().to_vec()),
                field(b"DATA", 0.2f32.to_le_bytes().to_vec()),
                field(b"ENIT", [2u32.to_le_bytes(), 3u32.to_le_bytes()].concat()),
            ],
        );
        let def = parse_item_def(&rec, Some(&strings)).unwrap();
        assert_eq!(def.name, "Sweetroll");
        let def = parse_item_def(&rec, None).unwrap();
        assert_eq!(def.name, "Food Sweetroll");
    }

    #[test]
    fn weapon_parses_value_weight_damage() {
        // IronSword's real bytes (item_probe): value 25, weight 9,
        // damage 7.
        let mut data = value_weight(25, 9.0);
        data.extend_from_slice(&7u16.to_le_bytes());
        let rec = record(
            b"WEAP",
            0x0001_2eb7,
            vec![str_field(b"EDID", "IronSword"), field(b"DATA", data)],
        );
        let def = parse_item_def(&rec, None).unwrap();
        assert_eq!(def.kind, ItemKind::Weapon);
        assert_eq!(def.name, "Iron Sword");
        assert_eq!(def.value, 25);
        assert_eq!(def.weight, 9.0);
        assert_eq!(def.damage, 7);
    }

    #[test]
    fn armor_parses_rating_and_slot() {
        // Iron Cuirass real bytes: value 125, weight 30, DNAM
        // 2500 (rating 25), BOD2 slots 0x04 (body).
        let rec = record(
            b"ARMO",
            0x0001_2e49,
            vec![
                str_field(b"EDID", "ArmorIronCuirass"),
                field(b"DATA", value_weight(125, 30.0)),
                field(b"DNAM", 2500u32.to_le_bytes().to_vec()),
                field(b"BOD2", [4u32.to_le_bytes(), 1u32.to_le_bytes()].concat()),
            ],
        );
        let def = parse_item_def(&rec, None).unwrap();
        assert_eq!(def.kind, ItemKind::Armor);
        assert_eq!(def.value, 125);
        assert_eq!(def.weight, 30.0);
        assert_eq!(def.armor_rating, 25.0);
        assert_eq!(def.slot, Some(EquipSlot::Body));
        // Helmet BOD2 0x1002 -> Head; boots 0x80 -> Feet;
        // gauntlets 0x08 -> Hands; shield 0x200 -> Shield.
        assert_eq!(slot_from_bod2(0x1002), Some(EquipSlot::Head));
        assert_eq!(slot_from_bod2(0x80), Some(EquipSlot::Feet));
        assert_eq!(slot_from_bod2(0x08), Some(EquipSlot::Hands));
        assert_eq!(slot_from_bod2(0x200), Some(EquipSlot::Shield));
        // Ring (0x40) / amulet (0x20) have no M17 slot.
        assert_eq!(slot_from_bod2(0x40), None);
        assert_eq!(slot_from_bod2(0x20), None);
    }

    #[test]
    fn alch_food_vs_potion_from_enit_flags() {
        // Bread: DATA weight 0.2, ENIT value 2, flags 3 (food).
        // Potion: DATA weight 0.5, ENIT value 17, flags 0.
        let food = record(
            b"ALCH",
            0x0006_5c97,
            vec![
                str_field(b"EDID", "FoodBread01A"),
                field(b"DATA", 0.2f32.to_le_bytes().to_vec()),
                field(b"ENIT", [2u32.to_le_bytes(), 3u32.to_le_bytes()].concat()),
            ],
        );
        let def = parse_item_def(&food, None).unwrap();
        assert_eq!(def.kind, ItemKind::Food);
        assert_eq!(def.value, 2);
        assert_eq!(def.weight, 0.2);
        let potion = record(
            b"ALCH",
            0x0003_eadd,
            vec![
                str_field(b"EDID", "RestoreHealth01"),
                field(b"DATA", 0.5f32.to_le_bytes().to_vec()),
                field(b"ENIT", [17u32.to_le_bytes(), 0u32.to_le_bytes()].concat()),
            ],
        );
        let def = parse_item_def(&potion, None).unwrap();
        assert_eq!(def.kind, ItemKind::Potion);
        assert_eq!(def.value, 17);
        assert_eq!(def.weight, 0.5);
    }

    #[test]
    fn book_value_and_weight_at_offsets_8_and_12() {
        // Book2CommonGreatWar real bytes: flags 0, teaches -1,
        // value 6, weight 1.0.
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&(-1i32).to_le_bytes());
        data.extend_from_slice(&6i32.to_le_bytes());
        data.extend_from_slice(&1.0f32.to_le_bytes());
        let rec = record(
            b"BOOK",
            0x000f_456d,
            vec![
                str_field(b"EDID", "Book2CommonGreatWar"),
                field(b"DATA", data),
            ],
        );
        let def = parse_item_def(&rec, None).unwrap();
        assert_eq!(def.kind, ItemKind::Book);
        assert_eq!(def.value, 6);
        assert_eq!(def.weight, 1.0);
    }

    #[test]
    fn non_items_and_edid_less_records_are_skipped() {
        let stat = record(b"STAT", 0x1234, vec![str_field(b"EDID", "FarmTable01")]);
        assert!(parse_item_def(&stat, None).is_none());
        let no_edid = record(b"MISC", 0x1235, vec![field(b"DATA", value_weight(1, 1.0))]);
        assert!(parse_item_def(&no_edid, None).is_none());
    }

    fn test_def(base: u32, kind: ItemKind, slot: Option<EquipSlot>) -> ItemDef {
        ItemDef {
            base_form_id: base,
            name: format!("Item {base:#x}"),
            kind,
            value: 10,
            weight: 1.0,
            damage: 5,
            speed: 1.0,
            reach: 1.0,
            is_bow: false,
            projectile_model: None,
            projectile_speed: 0.0,
            projectile_gravity: 0.0,
            armor_rating: 10.0,
            slot,
            worn_model: None,
            description: None,
            effects: Vec::new(),
            ground_model: None,
        }
    }

    #[test]
    fn inventory_add_stacks_and_gold_is_a_counter() {
        let mut inv = Inventory::default();
        inv.add(0x100, 2);
        inv.add(0x100, 3);
        assert_eq!(inv.count(0x100), 5);
        assert_eq!(inv.stacks.len(), 1);
        inv.add(GOLD_FORM_ID, 7);
        assert_eq!(inv.gold, 7);
        assert_eq!(inv.count(GOLD_FORM_ID), 7);
        // Gold never becomes a stack.
        assert!(inv.stacks.iter().all(|s| s.base_form_id != GOLD_FORM_ID));
        inv.add(0x200, 0);
        assert_eq!(inv.count(0x200), 0);
    }

    #[test]
    fn inventory_remove_guards_and_cleans_up() {
        let mut inv = Inventory::default();
        inv.add(0x100, 3);
        assert!(!inv.remove(0x100, 5)); // too many: nothing removed
        assert_eq!(inv.count(0x100), 3);
        assert!(inv.remove(0x100, 3));
        assert_eq!(inv.count(0x100), 0);
        assert!(inv.stacks.is_empty());
        assert!(!inv.remove(0x999, 1)); // absent
    }

    #[test]
    fn equip_toggles_and_replaces_per_slot() {
        let mut inv = Inventory::default();
        let sword = test_def(0x100, ItemKind::Weapon, None);
        let axe = test_def(0x101, ItemKind::Weapon, None);
        let cuirass = test_def(0x200, ItemKind::Armor, Some(EquipSlot::Body));
        let helmet = test_def(0x201, ItemKind::Armor, Some(EquipSlot::Head));
        let ring = test_def(0x202, ItemKind::Armor, None);
        // Not carried: not equippable.
        assert_eq!(inv.equip(0x100, &sword), EquipOutcome::NotEquippable);
        inv.add(0x100, 1);
        inv.add(0x101, 1);
        inv.add(0x200, 1);
        inv.add(0x201, 1);
        inv.add(0x202, 1);
        assert_eq!(inv.equip(0x100, &sword), EquipOutcome::Equipped);
        assert!(inv.is_equipped(0x100));
        // Equipping the axe replaces the sword.
        assert_eq!(inv.equip(0x101, &axe), EquipOutcome::Equipped);
        assert!(!inv.is_equipped(0x100));
        assert!(inv.is_equipped(0x101));
        // Toggling the axe unequips it.
        assert_eq!(inv.equip(0x101, &axe), EquipOutcome::Unequipped);
        assert_eq!(inv.equipped_weapon, None);
        // Armor slots are independent.
        assert_eq!(inv.equip(0x200, &cuirass), EquipOutcome::Equipped);
        assert_eq!(inv.equip(0x201, &helmet), EquipOutcome::Equipped);
        assert!(inv.is_equipped(0x200));
        assert!(inv.is_equipped(0x201));
        // A slotless armor piece (ring) is not equippable in M17.
        assert_eq!(inv.equip(0x202, &ring), EquipOutcome::NotEquippable);
        // Removing an equipped item unequips it.
        assert!(inv.remove(0x200, 1));
        assert!(!inv.is_equipped(0x200));
    }

    #[test]
    fn use_consumes_food_and_potions_only() {
        let mut inv = Inventory::default();
        let bread = test_def(0x300, ItemKind::Food, None);
        let potion = test_def(0x301, ItemKind::Potion, None);
        let sword = test_def(0x100, ItemKind::Weapon, None);
        inv.add(0x300, 2);
        inv.add(0x301, 1);
        inv.add(0x100, 1);
        assert_eq!(inv.use_item(0x300, &bread), UseOutcome::Consumed);
        assert_eq!(inv.count(0x300), 1);
        assert_eq!(inv.use_item(0x301, &potion), UseOutcome::Consumed);
        assert_eq!(inv.count(0x301), 0);
        assert_eq!(inv.use_item(0x100, &sword), UseOutcome::NotUsable);
        assert_eq!(inv.count(0x100), 1);
        // Nothing left to consume.
        assert_eq!(inv.use_item(0x301, &potion), UseOutcome::NotUsable);
    }

    #[test]
    fn total_weight_and_value_sum_stacks() {
        let mut catalog = ItemCatalog::default();
        catalog
            .defs
            .insert(0x100, test_def(0x100, ItemKind::Weapon, None));
        catalog.defs.insert(
            0x200,
            ItemDef {
                weight: 30.0,
                value: 125,
                ..test_def(0x200, ItemKind::Armor, Some(EquipSlot::Body))
            },
        );
        let mut inv = Inventory::default();
        inv.add(0x100, 2); // 2 x 1.0 wt, 2 x 10g
        inv.add(0x200, 1); // 30 wt, 125g
        inv.add(GOLD_FORM_ID, 50); // weighs nothing, not counted
        assert_eq!(inv.total_weight(&catalog), 32.0);
        assert_eq!(inv.total_value(&catalog), 145);
    }

    #[test]
    fn lvli_resolution_is_deterministic_and_level_gated() {
        let mut catalog = ItemCatalog::default();
        catalog.lvli.insert(
            0x500,
            vec![
                LvliEntry {
                    level: 1,
                    item_form_id: 0x600,
                    count: 2,
                },
                LvliEntry {
                    level: 5,
                    item_form_id: 0x601,
                    count: 1,
                },
                LvliEntry {
                    level: 1,
                    item_form_id: 0x602,
                    count: 3,
                },
            ],
        );
        // Level 1: only the two level-1 entries are eligible; the
        // pick is deterministic for one container...
        let first = resolve_lvli(&catalog, 0x500, 1, 0x777);
        let second = resolve_lvli(&catalog, 0x500, 1, 0x777);
        assert_eq!(first, second);
        assert_eq!(first.len(), 1);
        assert!(first[0].0 == 0x600 || first[0].0 == 0x602);
        assert_ne!(first[0].0, 0x601); // level 5, gated out
                                       // ...and different containers can resolve differently.
        let mut seen = HashSet::new();
        for container in 0..20u32 {
            seen.insert(resolve_lvli(&catalog, 0x500, 1, container)[0].0);
        }
        assert!(seen.len() > 1, "one container ref varied the pick");
        // Level 5 opens the level-5 entry too.
        let at_five = resolve_lvli(&catalog, 0x500, 5, 0x777);
        assert_eq!(at_five.len(), 1);
        // A missing list resolves to nothing.
        assert!(resolve_lvli(&catalog, 0x999, 1, 0x777).is_empty());
    }

    #[test]
    fn outfit_lvli_takes_all_eligible_entries() {
        // ArmorIronSetAll (real data): 4 iron pieces, all level 1,
        // LVLF 0x04 (Use All).
        let mut catalog = ItemCatalog::default();
        catalog.lvli.insert(
            0x500,
            vec![
                LvliEntry {
                    level: 1,
                    item_form_id: 0x600,
                    count: 1,
                },
                LvliEntry {
                    level: 1,
                    item_form_id: 0x601,
                    count: 1,
                },
                LvliEntry {
                    level: 5,
                    item_form_id: 0x602,
                    count: 1,
                },
            ],
        );
        catalog.lvli_meta.insert(
            0x500,
            LvliMeta {
                flags: 0x04,
                chance_none: 0,
            },
        );
        // Level 1 takes both level-1 pieces, not the level-5 one.
        assert_eq!(
            resolve_outfit_lvli(&catalog, 0x500, 1, 0),
            vec![0x600, 0x601]
        );
        // Level 5 takes all three.
        assert_eq!(
            resolve_outfit_lvli(&catalog, 0x500, 5, 0),
            vec![0x600, 0x601, 0x602]
        );
        // Nested Use-All lists resolve take-all too.
        catalog.lvli.insert(
            0x501,
            vec![LvliEntry {
                level: 1,
                item_form_id: 0x500,
                count: 1,
            }],
        );
        catalog.lvli_meta.insert(
            0x501,
            LvliMeta {
                flags: 0x04,
                chance_none: 0,
            },
        );
        assert_eq!(
            resolve_outfit_lvli(&catalog, 0x501, 1, 0),
            vec![0x600, 0x601]
        );
        // A missing list resolves to nothing.
        assert!(resolve_outfit_lvli(&catalog, 0x999, 1, 0).is_empty());
    }

    #[test]
    fn outfit_lvli_pick_one_takes_exactly_one() {
        // A variety list (no Use All flag) is ONE deterministic
        // pick, not the whole list.
        let mut catalog = ItemCatalog::default();
        catalog.lvli.insert(
            0x510,
            vec![
                LvliEntry {
                    level: 1,
                    item_form_id: 0x600,
                    count: 1,
                },
                LvliEntry {
                    level: 1,
                    item_form_id: 0x601,
                    count: 1,
                },
                LvliEntry {
                    level: 1,
                    item_form_id: 0x602,
                    count: 1,
                },
            ],
        );
        catalog.lvli_meta.insert(
            0x510,
            LvliMeta {
                flags: 0,
                chance_none: 0,
            },
        );
        let first = resolve_outfit_lvli(&catalog, 0x510, 1, 0);
        assert_eq!(first, vec![0x601]);
        // Deterministic: the same list resolves the same way.
        assert_eq!(resolve_outfit_lvli(&catalog, 0x510, 1, 0), first);
    }

    #[test]
    fn outfit_lvli_pick_one_honors_chance_none() {
        let mut catalog = ItemCatalog::default();
        catalog.lvli.insert(
            0x520,
            vec![LvliEntry {
                level: 1,
                item_form_id: 0x600,
                count: 1,
            }],
        );
        // Chance None 100: the roll never yields an item.
        catalog.lvli_meta.insert(
            0x520,
            LvliMeta {
                flags: 0,
                chance_none: 100,
            },
        );
        assert!(resolve_outfit_lvli(&catalog, 0x520, 1, 0).is_empty());
        // Chance None 0: the pick lands.
        catalog.lvli_meta.insert(
            0x520,
            LvliMeta {
                flags: 0,
                chance_none: 0,
            },
        );
        assert_eq!(resolve_outfit_lvli(&catalog, 0x520, 1, 0), vec![0x600]);
        // Use All ignores chance entirely.
        catalog.lvli_meta.insert(
            0x520,
            LvliMeta {
                flags: 0x04,
                chance_none: 100,
            },
        );
        assert_eq!(resolve_outfit_lvli(&catalog, 0x520, 1, 0), vec![0x600]);
    }

    #[test]
    fn outfit_lvli_nested_lists_apply_their_own_flag() {
        // Use-All outer list holding one pick-one inner list and
        // one direct item: the inner list contributes ONE pick,
        // the direct item is taken.
        let mut catalog = ItemCatalog::default();
        catalog.lvli.insert(
            0x530,
            vec![
                LvliEntry {
                    level: 1,
                    item_form_id: 0x531,
                    count: 1,
                },
                LvliEntry {
                    level: 1,
                    item_form_id: 0x640,
                    count: 1,
                },
            ],
        );
        catalog.lvli_meta.insert(
            0x530,
            LvliMeta {
                flags: 0x04,
                chance_none: 0,
            },
        );
        catalog.lvli.insert(
            0x531,
            vec![
                LvliEntry {
                    level: 1,
                    item_form_id: 0x650,
                    count: 1,
                },
                LvliEntry {
                    level: 1,
                    item_form_id: 0x651,
                    count: 1,
                },
            ],
        );
        catalog.lvli_meta.insert(
            0x531,
            LvliMeta {
                flags: 0,
                chance_none: 0,
            },
        );
        let resolved = resolve_outfit_lvli(&catalog, 0x530, 1, 0);
        assert_eq!(resolved.len(), 2);
        assert!(resolved.contains(&0x640));
        assert!(
            resolved.contains(&0x650) ^ resolved.contains(&0x651),
            "exactly one inner pick: {resolved:?}"
        );
    }

    #[test]
    fn lvli_nesting_recurses_with_count_multiplied() {
        let mut catalog = ItemCatalog::default();
        catalog.lvli.insert(
            0x500,
            vec![LvliEntry {
                level: 1,
                item_form_id: 0x501,
                count: 3,
            }],
        );
        catalog.lvli.insert(
            0x501,
            vec![LvliEntry {
                level: 1,
                item_form_id: 0x600,
                count: 2,
            }],
        );
        // Outer count 3 x inner count 2 = 6.
        assert_eq!(resolve_lvli(&catalog, 0x500, 1, 0), vec![(0x600, 6)]);
        // A self-referencing list stops at the depth cap instead
        // of recursing forever.
        catalog.lvli.insert(
            0x502,
            vec![LvliEntry {
                level: 1,
                item_form_id: 0x502,
                count: 1,
            }],
        );
        assert!(resolve_lvli(&catalog, 0x502, 1, 0).is_empty());
    }

    #[test]
    fn tab_for_kind_maps_every_kind() {
        assert_eq!(tab_for_kind(ItemKind::Weapon), InventoryTab::Weapons);
        assert_eq!(tab_for_kind(ItemKind::Armor), InventoryTab::Apparel);
        assert_eq!(tab_for_kind(ItemKind::Potion), InventoryTab::Potions);
        assert_eq!(tab_for_kind(ItemKind::Food), InventoryTab::Food);
        assert_eq!(
            tab_for_kind(ItemKind::Ingredient),
            InventoryTab::Ingredients
        );
        assert_eq!(tab_for_kind(ItemKind::Book), InventoryTab::Books);
        assert_eq!(tab_for_kind(ItemKind::Key), InventoryTab::Keys);
        assert_eq!(tab_for_kind(ItemKind::Scroll), InventoryTab::Scrolls);
        // Ammo, soul gems, notes, and misc all file under Misc.
        for kind in [
            ItemKind::Ammo,
            ItemKind::Misc,
            ItemKind::SoulGem,
            ItemKind::Note,
        ] {
            assert_eq!(tab_for_kind(kind), InventoryTab::Misc);
        }
    }

    #[test]
    fn ten_categories_including_scrolls() {
        assert_eq!(InventoryTab::ALL.len(), 10);
        assert!(InventoryTab::ALL.contains(&InventoryTab::Scrolls));
        assert_eq!(InventoryTab::Scrolls.label(), "SCROLLS");
        assert_eq!(
            InventoryTab::from_name("scrolls"),
            Some(InventoryTab::Scrolls)
        );
    }

    #[test]
    fn layout_constants_match_the_swf() {
        // The .swf-proven geometry, pinned against drift:
        // categories x=20, items x=230, card at (532, 434.3),
        // bottom bar at y=645 (1280x720 stage).
        assert_eq!(CATEGORY_LIST_X, 20.0);
        assert_eq!(ITEM_LIST_X, 230.0);
        assert_eq!(CARD_X, 532.0);
        assert_eq!(CARD_Y, 434.3);
        assert_eq!(BOTTOM_BAR_Y, 645.0);
        assert_eq!(MODEL_VIEW_W, 740.0);
        assert_eq!(STAGE_WIDTH, 1280.0);
        assert_eq!(STAGE_HEIGHT, 720.0);
    }

    #[test]
    fn centered_scroll_y_centers_and_clamps() {
        // A long list (700 px content, 617 viewport): the
        // selected row at content offset 350 centers at 315.
        assert_eq!(centered_scroll_y(350.0, 315.0, 700.0, 617.0), 35.0);
        // Near the top: the list cannot scroll above 0.
        assert_eq!(centered_scroll_y(0.0, 315.0, 700.0, 617.0), 0.0);
        assert_eq!(centered_scroll_y(100.0, 315.0, 700.0, 617.0), 0.0);
        // Near the bottom: clamped to the content's end (700 -
        // 617 = 83), not the centered 385.
        assert_eq!(centered_scroll_y(700.0, 315.0, 700.0, 617.0), 83.0);
        // A short list never scrolls.
        assert_eq!(centered_scroll_y(100.0, 315.0, 400.0, 617.0), 0.0);
    }

    #[test]
    fn card_is_wide_for_effects_and_descriptions() {
        let sword = named_def(0x100, "Iron Sword", ItemKind::Weapon);
        assert!(!card_is_wide(&sword));
        let potion = ItemDef {
            effects: vec!["Restore Health".to_string()],
            ..named_def(0x400, "Potion", ItemKind::Potion)
        };
        assert!(card_is_wide(&potion));
        let ingredient = ItemDef {
            effects: vec!["Restore Health".to_string()],
            ..named_def(0x401, "Blue Mountain Flower", ItemKind::Ingredient)
        };
        assert!(card_is_wide(&ingredient));
        let book = ItemDef {
            description: Some("Daedra Heart".to_string()),
            ..named_def(0x300, "Atronach Forge Recipe", ItemKind::Book)
        };
        assert!(card_is_wide(&book));
        let plain_book = named_def(0x301, "The Great War", ItemKind::Book);
        assert!(!card_is_wide(&plain_book));
    }

    #[test]
    fn primary_action_label_is_title_case() {
        assert_eq!(
            primary_action_label(&named_def(0x100, "Iron Sword", ItemKind::Weapon)),
            Some("Equip")
        );
        assert_eq!(
            primary_action_label(&named_def(0x200, "Iron Cuirass", ItemKind::Armor)),
            Some("Equip")
        );
        assert_eq!(
            primary_action_label(&named_def(0x300, "Bread", ItemKind::Food)),
            Some("Eat")
        );
        assert_eq!(
            primary_action_label(&named_def(0x400, "Potion", ItemKind::Potion)),
            Some("Drink")
        );
        assert_eq!(
            primary_action_label(&named_def(0x500, "Book", ItemKind::Book)),
            None
        );
        assert_eq!(
            primary_action_label(&named_def(0x501, "Tankard", ItemKind::Misc)),
            None
        );
    }

    #[test]
    fn item_kind_type_labels() {
        assert_eq!(ItemKind::Weapon.type_label(), "WEAPON");
        assert_eq!(ItemKind::Armor.type_label(), "APPAREL");
        assert_eq!(ItemKind::Food.type_label(), "FOOD");
        assert_eq!(ItemKind::Potion.type_label(), "POTION");
        assert_eq!(ItemKind::Book.type_label(), "BOOK");
        assert_eq!(ItemKind::Ingredient.type_label(), "INGREDIENT");
        assert_eq!(ItemKind::Scroll.type_label(), "SCROLL");
        assert_eq!(ItemKind::SoulGem.type_label(), "SOUL GEM");
    }

    #[test]
    fn inventory_tab_names_round_trip() {
        for tab in InventoryTab::ALL {
            assert_eq!(
                InventoryTab::from_name(&tab.label().to_lowercase()),
                Some(tab)
            );
        }
        assert_eq!(InventoryTab::from_name("nope"), None);
        assert_eq!(InventoryTab::from_name(""), None);
    }

    fn named_def(base: u32, name: &str, kind: ItemKind) -> ItemDef {
        ItemDef {
            name: name.to_string(),
            ..test_def(base, kind, None)
        }
    }

    #[test]
    fn visible_stacks_filters_by_tab_and_sorts_by_name() {
        let mut catalog = ItemCatalog::default();
        catalog
            .defs
            .insert(0x100, named_def(0x100, "Iron Sword", ItemKind::Weapon));
        catalog
            .defs
            .insert(0x101, named_def(0x101, "Iron Dagger", ItemKind::Weapon));
        catalog
            .defs
            .insert(0x200, named_def(0x200, "Iron Cuirass", ItemKind::Armor));
        catalog
            .defs
            .insert(0x300, named_def(0x300, "Sweetroll", ItemKind::Food));
        let mut inv = Inventory::default();
        // Scrambled insertion order: the list sorts by name.
        inv.add(0x100, 1);
        inv.add(0x300, 2);
        inv.add(0x200, 1);
        inv.add(0x101, 1);
        let names = |tab: InventoryTab| {
            visible_stacks(&inv, &catalog, tab)
                .iter()
                .map(|s| catalog.def(s.base_form_id).unwrap().name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(InventoryTab::All),
            vec!["Iron Cuirass", "Iron Dagger", "Iron Sword", "Sweetroll"]
        );
        assert_eq!(
            names(InventoryTab::Weapons),
            vec!["Iron Dagger", "Iron Sword"]
        );
        assert_eq!(names(InventoryTab::Apparel), vec!["Iron Cuirass"]);
        assert_eq!(names(InventoryTab::Food), vec!["Sweetroll"]);
        assert!(names(InventoryTab::Books).is_empty());
        assert!(names(InventoryTab::Keys).is_empty());
    }

    /// Build a synthetic strings file: header + directory +
    /// data, with the given (id, text) entries. `length_prefix`
    /// selects the DLSTRINGS/ILSTRINGS layout. (Same builder as
    /// dovah-strings' own tests.)
    fn build_strings_file(entries: &[(u32, &str)], length_prefix: bool) -> Vec<u8> {
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

    #[test]
    fn book_description_parses_from_cnam() {
        // The real AtrFrgDaedricRecipe16 CNAM (75140 -> "Daedra
        // Heart", book_desc_probe): a BOOK def carries it as its
        // description; without strings (or with CNAM 0) the
        // description is None.
        let longform = dovah_strings::StringsFile::from_bytes(
            &build_strings_file(&[(75140, "Daedra Heart")], true),
            dovah_strings::StringsKind::LengthPrefixed,
        )
        .unwrap();
        let strings = crate::dialogue::GameStrings {
            sets: vec![dovah_strings::StringsSet {
                general: dovah_strings::StringsFile::default(),
                dialogue: dovah_strings::StringsFile::default(),
                longform,
            }],
        };
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&(-1i32).to_le_bytes());
        data.extend_from_slice(&6i32.to_le_bytes());
        data.extend_from_slice(&1.0f32.to_le_bytes());
        let rec = record(
            b"BOOK",
            0x000e_2a8d,
            vec![
                str_field(b"EDID", "AtrFrgDaedricRecipe16"),
                field(b"DATA", data),
                field(b"CNAM", 75140u32.to_le_bytes().to_vec()),
            ],
        );
        let def = parse_item_def(&rec, Some(&strings)).unwrap();
        assert_eq!(def.description.as_deref(), Some("Daedra Heart"));
        let def = parse_item_def(&rec, None).unwrap();
        assert_eq!(def.description, None);
    }

    #[test]
    fn alch_effect_names_resolve_mgef_full() {
        // A potion with two EFIDs: the MGEF FULLs resolve to
        // "Restore Stamina" and "Damage Stamina Regeneration"
        // (the real MQ101JuniperMead effects, book_desc_probe).
        let general = dovah_strings::StringsFile::from_bytes(
            &build_strings_file(
                &[(11, "Restore Stamina"), (12, "Damage Stamina Regeneration")],
                false,
            ),
            dovah_strings::StringsKind::General,
        )
        .unwrap();
        let strings = crate::dialogue::GameStrings {
            sets: vec![dovah_strings::StringsSet {
                general,
                dialogue: dovah_strings::StringsFile::default(),
                longform: dovah_strings::StringsFile::default(),
            }],
        };
        let mgef_a = record(
            b"MGEF",
            0x0001_0001,
            vec![field(b"FULL", 11u32.to_le_bytes().to_vec())],
        );
        let mgef_b = record(
            b"MGEF",
            0x0001_0002,
            vec![field(b"FULL", 12u32.to_le_bytes().to_vec())],
        );
        let alch = record(
            b"ALCH",
            0x0003_0001,
            vec![
                str_field(b"EDID", "MQ101JuniperMead"),
                field(b"DATA", 0.5f32.to_le_bytes().to_vec()),
                field(b"ENIT", [17u32.to_le_bytes(), 0u32.to_le_bytes()].concat()),
                field(b"EFID", 0x0001_0001u32.to_le_bytes().to_vec()),
                field(b"EFID", 0x0001_0002u32.to_le_bytes().to_vec()),
            ],
        );
        let by_form_id: HashMap<u32, &Record> = [
            (0x0001_0001, &mgef_a),
            (0x0001_0002, &mgef_b),
            (0x0003_0001, &alch),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            alch_effect_names(&alch, &by_form_id, Some(&strings)),
            vec!["Restore Stamina", "Damage Stamina Regeneration"]
        );
        assert!(alch_effect_names(&alch, &by_form_id, None).is_empty());
    }

    #[test]
    fn ground_model_parses_from_modl() {
        // A weapon's MODL is its ground model (trailing NUL
        // stripped); an ARMO's MODL is an ARMA form ID, not a
        // path, so parse leaves its ground model for the catalog
        // build (MOD2/MOD4); a model-less record has none.
        let mut data = value_weight(25, 9.0);
        data.extend_from_slice(&7u16.to_le_bytes());
        let sword = record(
            b"WEAP",
            0x0001_2eb7,
            vec![
                str_field(b"EDID", "IronSword"),
                field(b"DATA", data),
                str_field(b"MODL", "Weapons\\Iron\\LongSword.nif"),
            ],
        );
        let def = parse_item_def(&sword, None).unwrap();
        assert_eq!(
            def.ground_model.as_deref(),
            Some("Weapons\\Iron\\LongSword.nif")
        );
        let cuirass = record(
            b"ARMO",
            0x0001_2e49,
            vec![
                str_field(b"EDID", "ArmorIronCuirass"),
                field(b"DATA", value_weight(125, 30.0)),
                field(b"DNAM", 2500u32.to_le_bytes().to_vec()),
                field(b"BOD2", [4u32.to_le_bytes(), 1u32.to_le_bytes()].concat()),
                field(b"MODL", 0x0001_2e4au32.to_le_bytes().to_vec()),
            ],
        );
        let def = parse_item_def(&cuirass, None).unwrap();
        assert_eq!(def.ground_model, None);
        let abstract_item = record(
            b"MISC",
            0x0001_2345,
            vec![
                str_field(b"EDID", "AbstractThing"),
                field(b"DATA", value_weight(1, 1.0)),
            ],
        );
        let def = parse_item_def(&abstract_item, None).unwrap();
        assert_eq!(def.ground_model, None);
    }

    /// A minimal app for the drop proof: real asset provider
    /// (test-data BSAs — the drop spawns the real sword mesh),
    /// a player standing at the origin, an inventory holding an
    /// equipped Iron Sword. The keyboard is driven directly
    /// (ButtonInput inserted by hand — no InputPlugin, so no
    /// system clears just_pressed before the drop system reads
    /// it).
    fn drop_test_app() -> App {
        use bevy::app::Update;
        use bevy::asset::{AssetApp, AssetPlugin};
        let mut app = App::new();
        app.add_plugins((bevy::MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>()
            .insert_resource(ButtonInput::<KeyCode>::default());
        let test_data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data");
        let provider = crate::assets::AssetProvider::new(&test_data);
        app.insert_resource(provider);
        let mut catalog = ItemCatalog {
            built: true,
            ..Default::default()
        };
        catalog.defs.insert(
            0x12eb7,
            ItemDef {
                name: "Iron Sword".to_string(),
                ground_model: Some("Weapons\\Iron\\LongSword.nif".to_string()),
                ..test_def(0x12eb7, ItemKind::Weapon, None)
            },
        );
        catalog.defs.insert(
            0x12345,
            ItemDef {
                name: "Abstract Thing".to_string(),
                ..test_def(0x12345, ItemKind::Misc, None)
            },
        );
        app.insert_resource(catalog);
        let mut inventory = Inventory::default();
        inventory.add(0x12eb7, 2);
        inventory.add(0x12345, 1);
        inventory.equipped_weapon = Some(0x12eb7);
        app.insert_resource(inventory)
            .insert_resource(InventoryOpen(true))
            .insert_resource(SelectedItem(Some(0x12eb7)))
            .insert_resource(SelectedTab::default())
            .insert_resource(crate::interact::Pickupables::default())
            .insert_resource(DroppedRefIds::default())
            .insert_resource(DollDirty::default());
        // The player: eye at 1.7 m, feet at y=0, facing -Z.
        app.world_mut().spawn((
            Transform::from_translation(Vec3::new(0.0, crate::player::EYE_HEIGHT, 0.0)),
            crate::scene::Player,
        ));
        app.add_systems(Update, inventory_drop_key);
        app
    }

    #[test]
    fn drop_spawns_a_pickupable_world_item() {
        let mut app = drop_test_app();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyR);
        app.update();
        // One sword left the bag; the equipped one came off.
        let inventory = app.world().resource::<Inventory>();
        assert_eq!(inventory.count(0x12eb7), 1);
        assert_eq!(inventory.equipped_weapon, None);
        // And the doll is marked dirty (the rebuild system is
        // not in this app, so the flag stays set).
        assert!(app.world().resource::<DollDirty>().0);
        // The pickup index holds the drop under the first
        // synthetic ref ID, at the player's feet, a step ahead
        // (-Z) — raised by the sword's below-origin extent
        // (0.023 m), so the blade rests ON the floor instead of
        // burying its lower edge.
        let pickupables = app.world().resource::<crate::interact::Pickupables>();
        assert_eq!(pickupables.items.len(), 1);
        let dropped = pickupables.items.get(&0xFF00_0000).unwrap();
        assert_eq!(dropped.base_form_id, 0x12eb7);
        assert_eq!(dropped.position.x, 0.0);
        assert_eq!(dropped.position.z, -0.8);
        assert!(
            (dropped.position.y - 0.023).abs() < 0.001,
            "drop rests on the floor: y = {}",
            dropped.position.y
        );
        assert_eq!(app.world().resource::<DroppedRefIds>().0, 0xFEFF_FFFF);
        // And the real sword mesh is in the world as scene
        // entities at that position (the pickup despawn matches
        // on it).
        let mut query = app
            .world_mut()
            .query_filtered::<&Transform, With<crate::scene::SceneEntity>>();
        let positions: Vec<Vec3> = query.iter(app.world()).map(|t| t.translation).collect();
        assert!(
            !positions.is_empty(),
            "the dropped sword spawned no scene entities"
        );
        assert!(
            positions
                .iter()
                .all(|p| p.x == 0.0 && p.z == -0.8 && (p.y - 0.023).abs() < 0.001),
            "spawned entities sit at the raised drop position: {positions:?}"
        );
    }

    #[test]
    fn mesh_min_y_reads_the_lowest_vertex() {
        use bevy::asset::RenderAssetUsages;
        use bevy::mesh::PrimitiveTopology;
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.0, 0.5, 0.0], [1.0, -0.108, 0.0], [0.0, 0.2, 1.0]],
        );
        assert_eq!(mesh_min_y(&mesh), Some(-0.108));
        // No positions: None.
        let empty = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        assert_eq!(mesh_min_y(&empty), None);
    }

    #[test]
    fn drop_refuses_items_without_a_ground_model() {
        let mut app = drop_test_app();
        // Select the model-less item and press R: nothing leaves
        // the bag, nothing registers as a pickupable.
        app.world_mut().resource_mut::<SelectedItem>().0 = Some(0x12345);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyR);
        app.update();
        let inventory = app.world().resource::<Inventory>();
        assert_eq!(inventory.count(0x12345), 1);
        assert!(app
            .world()
            .resource::<crate::interact::Pickupables>()
            .items
            .is_empty());
    }
}
