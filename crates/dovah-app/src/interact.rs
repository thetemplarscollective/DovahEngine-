//! Interaction (M17): USE picks up loose items and loots
//! containers. Doors keep priority — `door_use` (M15) runs first
//! in the chain, and these systems skip a press when a door is in
//! range, so one USE never both opens a door and grabs a tankard.
//!
//! Pickup works from the loaded cell data, not from entity tags:
//! `Pickupables` maps each pickupable ref to its Bevy position
//! and rotation, and the pickup despawns the spawned entities
//! matching BOTH. Position alone is not enough: the hanging
//! animals share a ring position (5–6 FLOR refs per ring), but
//! every one has its own yaw, and the rings themselves sit at a
//! third yaw — `flor_rot_probe` proved (position + rotation)
//! unique across the whole inn. The match is exact float
//! equality on the translation and a sub-milliradian angle on
//! the rotation: entity transforms are computed from the same
//! Skyrim placement by the same `to_bevy_*`, so they agree
//! bit-for-bit.
//!
//! Flora (FLOR) is harvestable: a hanging rabbit gives
//! FoodRabbit, a garlic braid gives Garlic, a coin purse gives
//! gold from its leveled list — the yield is the base's PFIG
//! field, carried on the catalog. Reach is measured from the
//! EYE (feet + 1.7 m): the hanging animals anchor 2.1–2.8 m
//! above the floor, so a feet-based vertical would leave them
//! unreachable even though the player can plainly reach them.
//!
//! Containers give their CNTO contents on USE (take-all; a
//! per-item container UI is a later milestone). Leveled lists
//! resolve through the catalog's deterministic resolver. A looted
//! container is removed from `Containers`, so it cannot be looted
//! twice. The merchant's hidden chest sits 2.5 m under the floor —
//! outside the 2 m vertical tolerance, so it stays out of reach,
//! as in the game.

use crate::cell::CellData;
use crate::coords;
use crate::doors;
use crate::inventory::{prettify_edid, resolve_lvli, Inventory, ItemCatalog};
use crate::scene::{Player, SceneEntity};
use crate::touch::TouchInputState;
use bevy::app::{App, Plugin};
use bevy::ecs::prelude::*;
use bevy::input::keyboard::KeyCode;
use bevy::input::ButtonInput;
use bevy::math::{Quat, Vec3};
use bevy::transform::components::Transform;
use std::collections::HashMap;

/// Base types the player can pick up. (ARMO is included — armor
/// lying in the world is lootable even though the inn has none
/// loose.)
pub const PICKUPABLE_TYPES: &[&str] = &[
    "MISC", "WEAP", "ARMO", "ALCH", "BOOK", "INGR", "AMMO", "KEYM", "SLGM", "NOTE", "SCRL",
];

/// Horizontal USE range for items/containers (same as doors).
const USE_RANGE_M: f32 = 2.5;
/// Vertical tolerance for a USE target, measured from the
/// EYE (feet + 1.7 m — reach is a camera thing, as in the
/// game): the merchant chest 2.5 m under the floor is 4.2 m
/// below the eye, so it stays out of reach, and the hanging
/// animals 2.1–2.8 m up are 0.4–1.1 m above it, so they are
/// reachable.
const USE_HEIGHT_M: f32 = 2.0;

/// One loose item in the current scene. (The ref form ID is the
/// map key in `Pickupables`, so it is not stored again here.)
#[derive(Debug, Clone)]
pub struct Pickupable {
    /// The item form ID to add — the base itself for a normal
    /// item, the PFIG yield (an item, or a leveled list for a
    /// coin purse) for a FLOR harvest.
    pub base_form_id: u32,
    /// Bevy-space position (metres) — half of the despawn key.
    pub position: Vec3,
    /// Bevy-space rotation — the other half. Hanging animals
    /// share a position and differ only by yaw.
    pub rotation: Quat,
    /// For a FLOR harvest, the plant's own display name (the
    /// real name from `.strings` when available, else the
    /// prettified EDID, e.g. "Hanging Rabbit 02") — the log
    /// names what was harvested, not the yield. `None` for a
    /// normal item, whose name comes from its def.
    pub harvest_name: Option<String>,
}

/// One container in the current scene. (The ref form ID is the
/// map key in `Containers`, so it is not stored again here.)
#[derive(Debug, Clone)]
pub struct Container {
    /// Bevy-space position (metres).
    pub position: Vec3,
    /// Raw CNTO contents (item form ID, count) — LVLIs resolve at
    /// take-time.
    pub contents: Vec<(u32, i32)>,
}

/// Loose items in the current scene, by ref form ID. Rebuilt by
/// `build_interactables` on every scene load; a picked-up item is
/// removed, so it cannot be picked up twice.
#[derive(Debug, Default, Resource)]
pub struct Pickupables {
    pub items: HashMap<u32, Pickupable>,
}

/// Containers in the current scene, by ref form ID. Rebuilt on
/// every scene load; a looted container is removed.
#[derive(Debug, Default, Resource)]
pub struct Containers {
    pub items: HashMap<u32, Container>,
}

/// The interact plugin: initializes the pickupable/container
/// indexes. The systems themselves are wired in lib.rs (the
/// scene-load chain builds the indexes; the USE chain drives
/// pickup/loot), so this plugin carries no systems — it exists
/// so `build_interactables` never meets a missing resource
/// (a required `ResMut` on an uninitialized resource panics the
/// system on its first run — the v0.2.0 lesson).
pub struct InteractPlugin;

impl Plugin for InteractPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Pickupables>()
            .init_resource::<Containers>();
    }
}

/// Build the interactables for the loaded scene: every pickupable
/// ref and every CONT ref with contents, from the cell data and
/// the catalog. Runs in the scene-load chain, after
/// `build_catalog` (the catalog holds the container contents).
/// M18: FLOR harvest names come from `.strings` when the plugin
/// and strings are available (the FLOR base's FULL), else the
/// prettified EDID.
pub fn build_interactables(
    cell: Option<Res<CellData>>,
    catalog: Option<Res<ItemCatalog>>,
    plugin: Option<Res<crate::SharedPlugin>>,
    strings: Option<Res<crate::dialogue::GameStrings>>,
    mut pickupables: ResMut<Pickupables>,
    mut containers: ResMut<Containers>,
) {
    let (Some(cell), Some(catalog)) = (cell, catalog) else {
        return;
    };
    if !catalog.built {
        return;
    }
    pickupables.items.clear();
    containers.items.clear();
    for object in &cell.objects {
        let position = coords::to_bevy_position(object.position);
        let rotation = coords::to_bevy_rotation(object.rotation);
        if PICKUPABLE_TYPES.contains(&object.base_type.as_str()) {
            pickupables.items.insert(
                object.ref_form_id,
                Pickupable {
                    base_form_id: object.base_form_id,
                    position,
                    rotation,
                    harvest_name: None,
                },
            );
        } else if object.base_type == "FLOR" {
            // A harvest: the ref gives its PFIG yield, not
            // itself. A FLOR without a yield (no PFIG on the
            // base) yields nothing — skipped, not crashed on.
            if let Some(yield_id) = catalog.flor_yield.get(&object.base_form_id) {
                // M18: the real name from the FLOR base's FULL
                // (`.strings`), falling back to the prettified
                // EDID when the strings or the record are
                // missing.
                let harvest_name = plugin
                    .as_ref()
                    .and_then(|p| p.0.find(object.base_form_id))
                    .and_then(|record| strings.as_ref().and_then(|s| s.name_of(record)))
                    .unwrap_or_else(|| prettify_edid(&object.base_edid));
                pickupables.items.insert(
                    object.ref_form_id,
                    Pickupable {
                        base_form_id: *yield_id,
                        position,
                        rotation,
                        harvest_name: Some(harvest_name),
                    },
                );
            }
        } else if object.base_type == "CONT" {
            if let Some(contents) = catalog.container_contents.get(&object.base_form_id) {
                containers.items.insert(
                    object.ref_form_id,
                    Container {
                        position,
                        contents: contents.clone(),
                    },
                );
            }
        }
    }
    crate::startup_log::log(&format!(
        "interactables built: {} pickupables, {} containers with contents",
        pickupables.items.len(),
        containers.items.len()
    ));
}

/// Horizontal distance + vertical tolerance test for a USE target.
/// The vertical is measured from the EYE (feet + 1.7 m), not the
/// feet — the hanging animals anchor 2.1–2.8 m up and must be
/// reachable. Pure — unit-tested. `pub(crate)`: the dialogue
/// module reuses the same range rule for NPCs (M18).
pub(crate) fn in_use_range(target: Vec3, feet: Vec3) -> bool {
    let dx = feet.x - target.x;
    let dz = feet.z - target.z;
    let horizontal = (dx * dx + dz * dz).sqrt();
    let eye_y = feet.y + crate::player::EYE_HEIGHT;
    let vertical = (eye_y - target.y).abs();
    horizontal <= USE_RANGE_M && vertical <= USE_HEIGHT_M
}

/// Nearest pickupable within USE range of the player's feet, or
/// `None`. Pure — unit-tested.
pub fn nearest_pickupable(items: &Pickupables, feet: Vec3) -> Option<u32> {
    let mut best: Option<(u32, f32)> = None;
    for (ref_form_id, item) in &items.items {
        if !in_use_range(item.position, feet) {
            continue;
        }
        let dx = feet.x - item.position.x;
        let dz = feet.z - item.position.z;
        let dist = dx * dx + dz * dz;
        let better = match best {
            None => true,
            Some((_, d)) => dist < d,
        };
        if better {
            best = Some((*ref_form_id, dist));
        }
    }
    best.map(|(id, _)| id)
}

/// Nearest container within USE range of the player's feet, or
/// `None`. Pure — unit-tested.
pub fn nearest_container(items: &Containers, feet: Vec3) -> Option<u32> {
    let mut best: Option<(u32, f32)> = None;
    for (ref_form_id, item) in &items.items {
        if !in_use_range(item.position, feet) {
            continue;
        }
        let dx = feet.x - item.position.x;
        let dz = feet.z - item.position.z;
        let dist = dx * dx + dz * dz;
        let better = match best {
            None => true,
            Some((_, d)) => dist < d,
        };
        if better {
            best = Some((*ref_form_id, dist));
        }
    }
    best.map(|(id, _)| id)
}

/// Whether a USE press happened this frame (E on keyboard, the USE
/// touch button). Shared by the pickup and container systems.
fn use_pressed(keyboard: &ButtonInput<KeyCode>, touch: Option<&TouchInputState>) -> bool {
    keyboard.just_pressed(KeyCode::KeyE) || touch.map(|t| t.use_pressed).unwrap_or(false)
}

/// Despawn every scene entity at a pickupable's exact position
/// AND rotation (sub-meshes share the ref transform). The
/// rotation half matters: hanging animals share a ring
/// position, and only the harvested one goes. Shared by
/// `pickup_use` (the live take) and the save system's
/// `apply_world_changes` (re-applying a take to a fresh scene).
/// Returns the number of entities despawned.
pub(crate) fn despawn_pickupable(
    commands: &mut Commands,
    scene_entities: &Query<(Entity, &Transform), With<SceneEntity>>,
    item: &Pickupable,
) -> usize {
    let mut despawned = 0;
    for (entity, transform) in scene_entities.iter() {
        if transform.translation == item.position
            && transform.rotation.angle_between(item.rotation) < 0.001
        {
            commands.entity(entity).despawn();
            despawned += 1;
        }
    }
    despawned
}

/// USE picks up the nearest loose item in range: the item enters
/// the inventory (gold to the counter) and its entities leave the
/// world. Skipped when a door is in range (`door_use` already
/// handled the press), when an NPC is in range (M18 — dialogue
/// outranks pickup), and while the inventory or a dialogue is
/// open.
#[allow(clippy::too_many_arguments)]
pub fn pickup_use(
    keyboard: Res<ButtonInput<KeyCode>>,
    touch: Option<Res<TouchInputState>>,
    player: Query<&Transform, With<Player>>,
    pickupables: Option<ResMut<Pickupables>>,
    doors: Option<Res<doors::Doors>>,
    talkable: Option<Res<crate::dialogue::TalkableNpcs>>,
    catalog: Option<Res<ItemCatalog>>,
    mut inventory: ResMut<Inventory>,
    inventory_open: Option<Res<crate::inventory::InventoryOpen>>,
    dialogue_open: Option<Res<crate::dialogue::DialogueOpen>>,
    pause_open: Option<Res<crate::pause::PauseOpen>>,
    character_open: Option<Res<crate::character::CharacterMenuOpen>>,
    mut commands: Commands,
    scene_entities: Query<(Entity, &Transform), With<SceneEntity>>,
    mut world_changes: Option<ResMut<crate::save::WorldChanges>>,
) {
    if !use_pressed(&keyboard, touch.as_deref()) {
        return;
    }
    if inventory_open.map(|o| o.0).unwrap_or(false) {
        return;
    }
    if dialogue_open.map(|o| o.0).unwrap_or(false) {
        return;
    }
    // UI milestone 1: same for the pause screen.
    if pause_open.map(|o| o.0).unwrap_or(false) {
        return;
    }
    // UI milestone 2: same for the character menu.
    if character_open.map(|o| o.0).unwrap_or(false) {
        return;
    }
    let Some(mut pickupables) = pickupables else {
        return;
    };
    let Some(catalog) = catalog else {
        return;
    };
    let Ok(player_transform) = player.single() else {
        return;
    };
    let feet = Vec3::new(
        player_transform.translation.x,
        player_transform.translation.y - crate::player::EYE_HEIGHT,
        player_transform.translation.z,
    );
    // Doors first: a door in range means `door_use` (earlier in
    // the chain) already spent this press. Then dialogue (M18):
    // an NPC in range means `dialogue_use` already spent it.
    if let Some(doors) = doors {
        if doors::nearest_door_in_range(&doors, feet).is_some() {
            return;
        }
    }
    if let Some(talkable) = talkable {
        if crate::dialogue::nearest_npc_in_range(&talkable, feet).is_some() {
            return;
        }
    }
    let Some(ref_form_id) = nearest_pickupable(&pickupables, feet) else {
        return;
    };
    let Some(item) = pickupables.items.remove(&ref_form_id) else {
        return;
    };
    // The take: a normal item is one of itself; a FLOR harvest
    // resolves its yield — a leveled list (coin purse gold)
    // through the catalog's deterministic resolver, a direct
    // item (rabbit meat, garlic) as one of itself.
    if catalog.lvli.contains_key(&item.base_form_id) {
        // Player level 1 until progression exists.
        for (id, count) in resolve_lvli(&catalog, item.base_form_id, 1, ref_form_id) {
            inventory.add(id, count);
        }
    } else {
        inventory.add(item.base_form_id, 1);
    }
    // The item leaves the world: despawn every scene entity at
    // its exact position AND rotation, and record the take so
    // the save system keeps it taken across loads and door
    // transitions (the ref form ID is globally unique — no
    // cell-scoping needed).
    let despawned = despawn_pickupable(&mut commands, &scene_entities, &item);
    if let Some(world) = world_changes.as_deref_mut() {
        world.picked_up.insert(ref_form_id);
    }
    let name = item.harvest_name.clone().unwrap_or_else(|| {
        catalog
            .def(item.base_form_id)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| format!("item 0x{:08x}", item.base_form_id))
    });
    crate::startup_log::log(&format!(
        "picked up {name} (ref 0x{ref_form_id:08x}, {despawned} entities despawned)"
    ));
}

/// USE loots the nearest container in range (take-all): every
/// CNTO entry resolves (leveled lists through the catalog) and
/// moves into the inventory. Skipped when a door, an NPC (M18 —
/// dialogue outranks containers), or a loose item is in range
/// — those have priority — and while the inventory or a
/// dialogue is open.
#[allow(clippy::too_many_arguments)]
pub fn container_use(
    keyboard: Res<ButtonInput<KeyCode>>,
    touch: Option<Res<TouchInputState>>,
    player: Query<&Transform, With<Player>>,
    pickupables: Option<Res<Pickupables>>,
    containers: Option<ResMut<Containers>>,
    doors: Option<Res<doors::Doors>>,
    talkable: Option<Res<crate::dialogue::TalkableNpcs>>,
    catalog: Option<Res<ItemCatalog>>,
    mut inventory: ResMut<Inventory>,
    inventory_open: Option<Res<crate::inventory::InventoryOpen>>,
    dialogue_open: Option<Res<crate::dialogue::DialogueOpen>>,
    pause_open: Option<Res<crate::pause::PauseOpen>>,
    character_open: Option<Res<crate::character::CharacterMenuOpen>>,
    mut world_changes: Option<ResMut<crate::save::WorldChanges>>,
) {
    if !use_pressed(&keyboard, touch.as_deref()) {
        return;
    }
    if inventory_open.map(|o| o.0).unwrap_or(false) {
        return;
    }
    if dialogue_open.map(|o| o.0).unwrap_or(false) {
        return;
    }
    // UI milestone 1: same for the pause screen.
    if pause_open.map(|o| o.0).unwrap_or(false) {
        return;
    }
    // UI milestone 2: same for the character menu.
    if character_open.map(|o| o.0).unwrap_or(false) {
        return;
    }
    let Some(mut containers) = containers else {
        return;
    };
    let Some(catalog) = catalog else {
        return;
    };
    let Ok(player_transform) = player.single() else {
        return;
    };
    let feet = Vec3::new(
        player_transform.translation.x,
        player_transform.translation.y - crate::player::EYE_HEIGHT,
        player_transform.translation.z,
    );
    // Doors, then NPCs (M18), then loose items — all outrank
    // containers.
    if let Some(doors) = doors {
        if doors::nearest_door_in_range(&doors, feet).is_some() {
            return;
        }
    }
    if let Some(talkable) = talkable {
        if crate::dialogue::nearest_npc_in_range(&talkable, feet).is_some() {
            return;
        }
    }
    if let Some(pickupables) = pickupables {
        if nearest_pickupable(&pickupables, feet).is_some() {
            return;
        }
    }
    let Some(ref_form_id) = nearest_container(&containers, feet) else {
        return;
    };
    let Some(container) = containers.items.remove(&ref_form_id) else {
        return;
    };
    // Record the loot so the save system keeps the container
    // empty across loads and door transitions.
    if let Some(world) = world_changes.as_deref_mut() {
        world.looted_containers.insert(ref_form_id);
    }
    // Player level 1 until progression exists.
    let mut taken: Vec<(u32, i32)> = Vec::new();
    for (item_id, count) in &container.contents {
        if catalog.lvli.contains_key(item_id) {
            for _ in 0..*count {
                taken.extend(resolve_lvli(&catalog, *item_id, 1, ref_form_id));
            }
        } else {
            taken.push((*item_id, *count));
        }
    }
    let mut summary: Vec<String> = Vec::new();
    for (item_id, count) in taken {
        if catalog.def(item_id).is_none() && item_id != crate::inventory::GOLD_FORM_ID {
            // An unresolvable entry (a missing base, a non-item):
            // skipped, not crashed on.
            continue;
        }
        inventory.add(item_id, count);
        let name = catalog
            .def(item_id)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| "Gold".to_string());
        summary.push(format!("{count}x {name}"));
    }
    crate::startup_log::log(&format!(
        "looted container 0x{ref_form_id:08x}: {}",
        if summary.is_empty() {
            "nothing".to_string()
        } else {
            summary.join(", ")
        }
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pickupables_at(positions: &[(u32, Vec3)]) -> Pickupables {
        Pickupables {
            items: positions
                .iter()
                .map(|(id, pos)| {
                    (
                        *id,
                        Pickupable {
                            base_form_id: *id + 0x1000,
                            position: *pos,
                            rotation: Quat::IDENTITY,
                            harvest_name: None,
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn use_range_is_horizontal_and_vertical() {
        let target = Vec3::new(10.0, 0.0, 5.0);
        assert!(in_use_range(target, Vec3::new(10.0, 0.0, 5.0)));
        assert!(in_use_range(target, Vec3::new(12.0, 0.0, 5.0)));
        assert!(!in_use_range(target, Vec3::new(13.0, 0.0, 5.0))); // too far
        assert!(!in_use_range(target, Vec3::new(10.0, 2.5, 5.0))); // too high
        assert!(in_use_range(target, Vec3::new(10.0, -1.5, 5.0))); // below, in range
                                                                   // The merchant chest: 2.5 m under the floor — 4.2 m
                                                                   // below the eye, out of reach.
        assert!(!in_use_range(
            Vec3::new(0.0, -2.5, 0.0),
            Vec3::new(0.0, 0.0, 0.0)
        ));
        // The vertical is measured from the eye (1.7 m): a
        // hanging rabbit anchors 2.14 m up and a garlic braid
        // 2.81 m up — both in reach, as in the game.
        assert!(in_use_range(Vec3::new(0.0, 2.14, 0.0), Vec3::ZERO));
        assert!(in_use_range(Vec3::new(0.0, 2.81, 0.0), Vec3::ZERO));
        assert!(!in_use_range(Vec3::new(0.0, 3.9, 0.0), Vec3::ZERO));
    }

    #[test]
    fn nearest_pickupable_picks_the_closest_in_range() {
        let items = pickupables_at(&[
            (0x1, Vec3::new(1.0, 0.0, 0.0)),
            (0x2, Vec3::new(2.0, 0.0, 0.0)),
            (0x3, Vec3::new(10.0, 0.0, 0.0)),
        ]);
        assert_eq!(nearest_pickupable(&items, Vec3::ZERO), Some(0x1));
        assert_eq!(nearest_pickupable(&items, Vec3::new(20.0, 0.0, 0.0)), None);
        // The closest is picked even when it is not first in the map.
        let items = pickupables_at(&[
            (0x1, Vec3::new(2.0, 0.0, 0.0)),
            (0x2, Vec3::new(0.5, 0.0, 0.0)),
        ]);
        assert_eq!(nearest_pickupable(&items, Vec3::ZERO), Some(0x2));
    }

    #[test]
    fn nearest_container_picks_the_closest_in_range() {
        let mut containers = Containers::default();
        containers.items.insert(
            0x1,
            Container {
                position: Vec3::new(1.0, 0.0, 0.0),
                contents: vec![],
            },
        );
        containers.items.insert(
            0x2,
            Container {
                position: Vec3::new(2.0, 0.0, 0.0),
                contents: vec![],
            },
        );
        assert_eq!(nearest_container(&containers, Vec3::ZERO), Some(0x1));
        assert_eq!(
            nearest_container(&containers, Vec3::new(20.0, 0.0, 0.0)),
            None
        );
    }

    /// The pickup proof, end to end in a minimal app: a player
    /// standing next to a tankard, a USE press (via the touch
    /// path — the deterministic way to drive it in a test), and
    /// after one update the tankard's entities are gone and the
    /// inventory holds it. The door-priority rule is covered too:
    /// with a door in range, the same press leaves the tankard
    /// alone.
    #[test]
    fn pickup_moves_the_item_from_world_to_inventory() {
        use bevy::app::{App, Update};
        use bevy::input::InputPlugin;

        fn test_app(with_door: bool) -> App {
            let mut app = App::new();
            app.add_plugins(bevy::MinimalPlugins)
                .add_plugins(InputPlugin)
                .insert_resource(Pickupables {
                    items: [(
                        0x600ee,
                        Pickupable {
                            base_form_id: 0x300,
                            position: Vec3::new(1.0, 0.0, 0.0),
                            rotation: Quat::IDENTITY,
                            harvest_name: None,
                        },
                    )]
                    .into_iter()
                    .collect(),
                })
                .insert_resource(Inventory::default())
                .insert_resource(crate::inventory::InventoryOpen(false));
            let mut catalog = ItemCatalog {
                built: true,
                ..Default::default()
            };
            catalog.defs.insert(
                0x300,
                crate::inventory::ItemDef {
                    base_form_id: 0x300,
                    name: "Basic Tankard 01".to_string(),
                    kind: crate::inventory::ItemKind::Misc,
                    value: 1,
                    weight: 0.5,
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
                },
            );
            app.insert_resource(catalog);
            if with_door {
                let mut doors = doors::Doors::default();
                doors.doors.insert(
                    0x13419,
                    doors::DoorRuntime {
                        ref_form_id: 0x13419,
                        anim: doors::DoorAnim {
                            open_angle: -0.3,
                            duration: 0.6,
                        },
                        width: 1.37,
                        plane_point: Vec3::new(0.5, 0.0, 0.0),
                        plane_normal: Vec3::Z,
                        lateral: Vec3::X,
                        base_y: 0.0,
                        has_xtel: true,
                        angle: 0.0,
                        target_angle: 0.0,
                    },
                );
                app.insert_resource(doors);
            }
            // The player, standing at the origin at eye height.
            app.world_mut().spawn((
                Transform::from_translation(Vec3::new(0.0, crate::player::EYE_HEIGHT, 0.0)),
                Player,
            ));
            // The tankard's entities: two sub-meshes at its
            // position, plus one unrelated entity elsewhere.
            app.world_mut().spawn((
                Transform::from_translation(Vec3::new(1.0, 0.0, 0.0)),
                SceneEntity,
            ));
            app.world_mut().spawn((
                Transform::from_translation(Vec3::new(1.0, 0.0, 0.0)),
                SceneEntity,
            ));
            app.world_mut().spawn((
                Transform::from_translation(Vec3::new(5.0, 0.0, 5.0)),
                SceneEntity,
            ));
            // The USE press, via the touch path.
            app.insert_resource(TouchInputState {
                use_pressed: true,
                ..Default::default()
            });
            app.add_systems(Update, pickup_use);
            app
        }

        // No door: the tankard is picked up.
        let mut app = test_app(false);
        app.update();
        let inventory = app.world().resource::<Inventory>();
        assert_eq!(inventory.count(0x300), 1);
        let pickupables = app.world().resource::<Pickupables>();
        assert!(!pickupables.items.contains_key(&0x600ee));
        // Both tankard entities despawned; the unrelated one stays.
        let mut query = app.world_mut().query::<&Transform>();
        let positions: Vec<Vec3> = query.iter(app.world()).map(|t| t.translation).collect();
        assert_eq!(positions.len(), 2); // player + unrelated entity
        assert!(positions.contains(&Vec3::new(5.0, 0.0, 5.0)));
        assert!(!positions.contains(&Vec3::new(1.0, 0.0, 0.0)));

        // A door in range: the press belongs to the door, the
        // tankard stays where it is.
        let mut app = test_app(true);
        app.update();
        let inventory = app.world().resource::<Inventory>();
        assert_eq!(inventory.count(0x300), 0);
        let pickupables = app.world().resource::<Pickupables>();
        assert!(pickupables.items.contains_key(&0x600ee));
    }

    /// The FLOR proof: two rabbits and their ring share one
    /// position and differ only by yaw. Harvesting one rabbit
    /// gives its PFIG yield (FoodRabbit) and despawns ONLY its
    /// entities — the other rabbit and the ring stay.
    #[test]
    fn flor_harvest_despawns_only_the_one_animal() {
        use bevy::app::{App, Update};
        use bevy::input::InputPlugin;
        use std::f32::consts::FRAC_PI_2;

        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .add_plugins(InputPlugin)
            .insert_resource(Pickupables {
                items: [(
                    0x600,
                    Pickupable {
                        base_form_id: 0x400,
                        position: Vec3::new(1.0, 2.14, 0.0),
                        rotation: Quat::IDENTITY,
                        harvest_name: Some("Hanging Rabbit 01".to_string()),
                    },
                )]
                .into_iter()
                .collect(),
            })
            .insert_resource(Inventory::default())
            .insert_resource(crate::inventory::InventoryOpen(false));
        let mut catalog = ItemCatalog {
            built: true,
            ..Default::default()
        };
        catalog.defs.insert(
            0x400,
            crate::inventory::ItemDef {
                base_form_id: 0x400,
                name: "Food Rabbit".to_string(),
                kind: crate::inventory::ItemKind::Food,
                value: 3,
                weight: 1.0,
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
            },
        );
        app.insert_resource(catalog);
        app.world_mut().spawn((
            Transform::from_translation(Vec3::new(0.0, crate::player::EYE_HEIGHT, 0.0)),
            Player,
        ));
        let ring_pos = Vec3::new(1.0, 2.14, 0.0);
        // The harvested rabbit (yaw 0), its neighbour (yaw 90°),
        // and the ring (yaw 45°) — all at one position.
        app.world_mut().spawn((
            Transform {
                translation: ring_pos,
                rotation: Quat::IDENTITY,
                ..Default::default()
            },
            SceneEntity,
        ));
        app.world_mut().spawn((
            Transform {
                translation: ring_pos,
                rotation: Quat::from_rotation_y(FRAC_PI_2),
                ..Default::default()
            },
            SceneEntity,
        ));
        app.world_mut().spawn((
            Transform {
                translation: ring_pos,
                rotation: Quat::from_rotation_y(FRAC_PI_2 / 2.0),
                ..Default::default()
            },
            SceneEntity,
        ));
        app.insert_resource(TouchInputState {
            use_pressed: true,
            ..Default::default()
        });
        app.add_systems(Update, pickup_use);
        app.update();

        let inventory = app.world().resource::<Inventory>();
        assert_eq!(inventory.count(0x400), 1);
        let pickupables = app.world().resource::<Pickupables>();
        assert!(!pickupables.items.contains_key(&0x600));
        // Player + neighbour rabbit + ring survive; the
        // harvested rabbit's entity is gone.
        let mut query = app.world_mut().query::<&Transform>();
        let transforms: Vec<Transform> = query.iter(app.world()).copied().collect();
        assert_eq!(transforms.len(), 3);
        assert!(
            !transforms
                .iter()
                .any(|t| t.translation == ring_pos
                    && t.rotation.angle_between(Quat::IDENTITY) < 0.001),
            "the harvested rabbit's entity must be despawned"
        );
        assert!(
            transforms.iter().any(|t| t.translation == ring_pos
                && t.rotation.angle_between(Quat::from_rotation_y(FRAC_PI_2)) < 0.001),
            "the neighbour rabbit must survive"
        );
        assert!(
            transforms.iter().any(|t| t.translation == ring_pos
                && t.rotation
                    .angle_between(Quat::from_rotation_y(FRAC_PI_2 / 2.0))
                    < 0.001),
            "the ring must survive"
        );
    }

    /// The coin-purse proof: a FLOR whose PFIG yield is a
    /// leveled list resolves through the catalog and pays out
    /// gold, and the purse leaves the world.
    #[test]
    fn coin_purse_harvest_resolves_the_gold_list() {
        use bevy::app::{App, Update};
        use bevy::input::InputPlugin;

        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .add_plugins(InputPlugin)
            .insert_resource(Pickupables {
                items: [(
                    0x601,
                    Pickupable {
                        base_form_id: 0x500,
                        position: Vec3::new(1.0, 0.0, 0.0),
                        rotation: Quat::IDENTITY,
                        harvest_name: Some("Coin Purse Small".to_string()),
                    },
                )]
                .into_iter()
                .collect(),
            })
            .insert_resource(Inventory::default())
            .insert_resource(crate::inventory::InventoryOpen(false));
        let mut catalog = ItemCatalog {
            built: true,
            ..Default::default()
        };
        catalog.lvli.insert(
            0x500,
            vec![crate::inventory::LvliEntry {
                level: 1,
                item_form_id: crate::inventory::GOLD_FORM_ID,
                count: 7,
            }],
        );
        app.insert_resource(catalog);
        app.world_mut().spawn((
            Transform::from_translation(Vec3::new(0.0, crate::player::EYE_HEIGHT, 0.0)),
            Player,
        ));
        app.world_mut().spawn((
            Transform::from_translation(Vec3::new(1.0, 0.0, 0.0)),
            SceneEntity,
        ));
        app.insert_resource(TouchInputState {
            use_pressed: true,
            ..Default::default()
        });
        app.add_systems(Update, pickup_use);
        app.update();

        let inventory = app.world().resource::<Inventory>();
        assert_eq!(inventory.count(crate::inventory::GOLD_FORM_ID), 7);
        let pickupables = app.world().resource::<Pickupables>();
        assert!(!pickupables.items.contains_key(&0x601));
        let mut query = app.world_mut().query::<&Transform>();
        let positions: Vec<Vec3> = query.iter(app.world()).map(|t| t.translation).collect();
        assert_eq!(positions.len(), 1); // only the player remains
    }
}
