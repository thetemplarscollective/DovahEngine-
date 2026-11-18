//! Archery: bow and arrow.
//!
//! The minimal loop: ATTACK with a bow equipped fires a visible
//! arrow (the PROJ flight model) that flies with gravity, hits
//! NPCs through the combat system, and sticks in the ground or
//! a wall on a miss. Damage is bow + ammo, per vanilla.
//!
//! What this milestone is NOT: there is no hold-to-draw (the
//! shot fires at full power on press — vanilla scales damage
//! and speed with draw time, which needs the draw animation);
//! no first-person bow viewmodel (no first-person hands exist);
//! no .hkx animation system; no NPC archers (arrows only fly
//! from the player); no arrow pickup (stuck arrows stay where
//! they land, up to a cap, and are not lootable yet); no
//! equipped-ammo slot (the first Ammo stack in the inventory
//! is fired). NPCs do not fight back (no AI/aggro system yet).
//!
//! Data (proven by the archery probes — see `archery_probe.log`,
//! `archery_proj_probe.log`, `arrow_axis_probe.log`):
//! ```text
//! AMMO DATA is 20 bytes (NOT the WEAP 10-byte layout — the old
//!   u16@8 read gave every arrow 0 damage):
//!   projectile form ID u32 @0, unknown u32 @4 (=4),
//!   damage f32 @8 (Iron 8, Steel 10, Daedric 24),
//!   value u32 @12, weight f32 @16 (0.1).
//! PROJ DATA is 92 bytes, identical for all 11 arrow types:
//!   gravity f32 @4 = 0.35 (a multiplier on world gravity),
//!   speed f32 @8 = 3600 u/s = 51.4 m/s,
//!   range f32 @12 = 60000 u = 857 m.
//!   PROJ MODL is the in-flight arrow model
//!   (IronArrowFlight.nif, OrcishArrowProjectile.nif, ...).
//! Bows are WEAP DNAM animation type u32 @0 == 7 (all bows;
//!   melee weapons are 1-6). Bow damage/speed: LongBow 6/1.0,
//!   HuntingBow 7/0.9375, DaedricBow 19/0.5.
//! ```
//!
//! The flight model (proven by the axis probe): the flight NIF
//! carries the arrow shape plus a 'trailShort' tracer (and a
//! 'Flow:0' remnant on some) — only the arrow shape is spawned.
//! The arrow's long axis is NIF Y, tip at low Y (the y<3 band
//! converges to a point, the y>55 band spreads) — after the
//! NIF->Bevy conversion the mesh points along Bevy +Z with the
//! tip at the entity origin and the tail 0.83 m behind it, so
//! the root rotation maps +Z onto the velocity and the entity
//! position IS the tip (what the hit checks use).
//!
//! Arrow sources in Riverwood: exactly one loose SteelArrow and
//! one loose LongBow, both in the inn's secret room; zero
//! arrows or bows outdoors. The player starts with neither.
//! The `--archery-test` seed grants a HuntingBow + IronArrows
//! for the desktop proof; the real game's source is vendors
//! (trading is a follow-up milestone).

use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions};
use std::collections::HashMap;

use crate::collision::ObstacleGrid;
use crate::combat::{
    apply_damage, fall_npc_parts, swing_cooldown_secs, Combatants, NpcPart, SwingCooldown,
};
use crate::dialogue::DialogueOpen;
use crate::inventory::{Inventory, InventoryOpen, ItemCatalog, ItemKind};
use crate::pause::PauseOpen;
use crate::player::{EYE_HEIGHT, GRAVITY};
use crate::save::WorldChanges;
use crate::scene::{FloorGrid, Player, SceneEntity, TerrainHeights};
use crate::touch::TouchInputState;

/// Combatant hit cylinder: radius around the feet, in metres.
/// (Melee uses a 1.5 m reach + 45-degree arc; an arrow needs a
/// body to hit, not a cone.)
pub const ARROW_BODY_RADIUS_M: f32 = 0.5;
/// Combatant hit cylinder: height above the feet, in metres.
pub const ARROW_BODY_HEIGHT_M: f32 = 1.9;
/// Longest single collision substep, in metres — smaller than
/// the obstacle cell (0.25 m) and the body radius, so a fast
/// arrow cannot tunnel through a wall or a body in one step.
pub const ARROW_SUBSTEP_M: f32 = 0.25;
/// Stuck arrows are capped (oldest despawn first) so a long
/// session cannot accumulate unbounded entities.
pub const MAX_STUCK_ARROWS: usize = 32;
/// Max flight distance, in metres: the PROJ range (60000 u)
/// for every arrow in Skyrim.esm.
pub const ARROW_MAX_RANGE_M: f32 = 60000.0 / crate::coords::SKYRIM_UNITS_PER_METER;
/// `--archery-test` distance from the target: far enough to see
/// the arrow fly, close enough to hit (the inn is ~20 m long).
pub const ARCHERY_TEST_DISTANCE_M: f32 = 8.0;
/// The arrow spawns this far ahead of the eye, along the aim —
/// clear of the player's own body, at the bow's position.
pub const ARROW_SPAWN_AHEAD_M: f32 = 0.5;
/// The seed's bow (HuntingBow 0x00013985: 7 damage, 0.9375
/// speed) and arrow (IronArrow 0x0001397d: 8 damage).
pub const SEED_BOW_FORM_ID: u32 = 0x0001_3985;
pub const SEED_ARROW_FORM_ID: u32 = 0x0001_397d;

/// A flying (or stuck) arrow. The entity position is the TIP —
/// the converted flight mesh points along +Z with the tip at
/// the origin, so the root rotation maps +Z onto the velocity
/// and the hit checks read the position directly.
#[derive(Component, Debug)]
pub struct Arrow {
    pub velocity: Vec3,
    /// Bow damage + ammo damage, per vanilla.
    pub damage: u16,
    pub bow_name: String,
    pub ammo_name: String,
    /// Downward acceleration (m/s^2): the PROJ gravity
    /// multiplier times the engine's gravity (0.35 x 22 = 7.7).
    pub gravity: f32,
    pub distance_flown: f32,
    pub stuck: bool,
    /// Spawn order (the stuck-arrow cap despawns the oldest).
    pub seq: u32,
    /// The flat mesh entities carrying this arrow's flight
    /// model (empty until `spawn_arrow_meshes` runs, or
    /// forever when there is no asset provider). Despawned
    /// with the arrow — see `sync_arrow_meshes`.
    pub mesh_entities: Vec<Entity>,
}

/// The flight-mesh entity for one arrow: a FLAT top-level
/// entity (not a child), mirroring the arrow root's transform
/// every frame via `sync_arrow_meshes`. Flat because the
/// engine's meshes are flat (scene.rs, `drop_item`) — and
/// because the first version parented the mesh under the
/// arrow root, and the stuck arrow then rendered invisible
/// after frame ~3 despite a correct main-world state (right
/// transform, `view_vis=true`, mesh present): the render
/// world stopped drawing the parented mesh once the root's
/// transform froze, for reasons the debug logs never pinned
/// down. The flat mesh has no parent, so no hierarchy
/// propagation is involved at all.
#[derive(Component, Debug)]
pub struct ArrowMesh {
    pub arrow: Entity,
}

/// Marker for an arrow whose flight mesh has not been attached
/// yet: `spawn_arrow_meshes` loads the PROJ flight NIF and
/// spawns a flat `ArrowMesh` entity for it. Physics and
/// rendering are decoupled on purpose — without an asset
/// provider (the headless App test) the arrow still flies and
/// hits, just invisibly.
#[derive(Component, Debug)]
pub struct NeedsArrowMesh {
    pub model_path: String,
}

/// Arrow spawn-order counter (the stuck-arrow cap).
#[derive(Debug, Default, Resource)]
pub struct ArrowSeq {
    pub next: u32,
}

/// The `--archery-test N` seed (desktop proof hook): teleport
/// the player 8 m from the seeded NPC, grant a HuntingBow +
/// N IronArrows, equip the bow, and queue N scripted shots
/// through the real fire system. The NPC is Orgnar
/// (NPC_ 0x00013479) when present, else the nearest combatant.
#[derive(Debug, Resource)]
pub struct ArcheryTestSeed {
    pub shots: u32,
    pub npc_base: Option<u32>,
}

/// Scripted shots still to fire (one per shot — the queue
/// decrements only when a shot actually fires, so presses
/// during the cooldown are not lost).
#[derive(Debug, Resource)]
pub struct ArcheryTestQueue {
    pub shots_remaining: u32,
}

/// `--archery-speed` override (m/s) for the screenshot path: a
/// slow arrow is visible across frames (at the real 51.4 m/s
/// it crosses the inn in one lavapipe frame). `None` = the
/// PROJ speed. A test hook only — never set on the phone path.
#[derive(Debug, Default, Resource)]
pub struct ArcherySpeedOverride(pub Option<f32>);

/// Archery plugin: resources + the fire / mesh / flight
/// systems, chained so a fired arrow gets its mesh and moves
/// in the same frame. Added by both the windowed and
/// screenshot paths (the screenshot path needs the fire system
/// for `--archery-test`).
pub struct ArcheryPlugin;

impl Plugin for ArcheryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ArrowSeq>()
            .init_resource::<ArcherySpeedOverride>()
            .add_systems(
                Update,
                (
                    archery_fire,
                    spawn_arrow_meshes,
                    arrow_flight,
                    sync_arrow_meshes,
                )
                    .chain()
                    .run_if(in_state(crate::AppState::InGame)),
            );
    }
}

// ----------------------------------------------------------------
// Pure helpers (unit-tested).
// ----------------------------------------------------------------

/// Vanilla arrow damage: bow damage + ammo damage.
pub fn arrow_damage(bow_damage: u16, ammo_damage: u16) -> u16 {
    bow_damage.saturating_add(ammo_damage)
}

/// Arrow gravity in m/s^2: the PROJ gravity multiplier times
/// the engine's gravity (0.35 x 22 = 7.7 m/s^2 for every
/// arrow in Skyrim.esm).
pub fn arrow_gravity_mps2(proj_gravity: f32) -> f32 {
    proj_gravity * GRAVITY
}

/// Substep count for one frame's travel: no substep longer
/// than ARROW_SUBSTEP_M, so a 51.4 m/s arrow cannot tunnel
/// through a 0.25 m obstacle cell in one 250 ms clamped frame.
pub fn flight_substeps(distance: f32) -> u32 {
    ((distance / ARROW_SUBSTEP_M).ceil() as u32).max(1)
}

/// Does the arrow tip hit this combatant? A vertical cylinder
/// around the feet: 0.5 m radius, 1.9 m tall.
pub fn hits_combatant(arrow: Vec3, feet: Vec3) -> bool {
    let dx = arrow.x - feet.x;
    let dz = arrow.z - feet.z;
    dx * dx + dz * dz <= ARROW_BODY_RADIUS_M * ARROW_BODY_RADIUS_M
        && arrow.y >= feet.y
        && arrow.y <= feet.y + ARROW_BODY_HEIGHT_M
}

/// Arrow orientation: the converted flight mesh points along
/// +Z (tip at the origin), so the root rotation maps +Z onto
/// the velocity direction.
pub fn arrow_rotation(velocity: Vec3) -> Quat {
    let dir = velocity.normalize_or_zero();
    if dir == Vec3::ZERO {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_arc(Vec3::Z, dir)
    }
}

/// Which shapes of a flight NIF are the arrow itself, by index
/// into the shape list: the single arrow shape — NOT the
/// 'trailShort' tracer or the 'Flow:0' remnant. Takes (name,
/// vertex count) pairs so the rule is testable without a NIF.
/// Prefer a shape named exactly "Arrow:0" (the AMMO ground
/// model); else every shape whose name contains "arrow"
/// (flight NIFs carry exactly one); else the largest shape by
/// vertex count.
pub fn select_flight_shapes(shapes: &[(&str, usize)]) -> Vec<usize> {
    if let Some(i) = shapes.iter().position(|(name, _)| *name == "Arrow:0") {
        return vec![i];
    }
    let named: Vec<usize> = shapes
        .iter()
        .enumerate()
        .filter(|(_, (name, _))| name.to_lowercase().contains("arrow"))
        .map(|(i, _)| i)
        .collect();
    if !named.is_empty() {
        return named;
    }
    shapes
        .iter()
        .enumerate()
        .max_by_key(|(_, (_, verts))| *verts)
        .map(|(i, _)| vec![i])
        .unwrap_or_default()
}

// ----------------------------------------------------------------
// The fire system.
// ----------------------------------------------------------------

/// Fire an arrow: ATTACK with a bow equipped consumes one
/// arrow from the first Ammo stack and spawns a flying Arrow
/// (mesh attached by `spawn_arrow_meshes` later in the chain).
/// The cooldown is the shared `SwingCooldown` — melee ticks it
/// every frame (its first statement), so this system only
/// reads and sets it; both systems are registered together.
/// No bow equipped, no arrows, or no flight model: the shot
/// does not fire (logged, not faked).
#[allow(clippy::too_many_arguments)]
pub fn archery_fire(
    mut commands: Commands,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    touch: Option<Res<TouchInputState>>,
    windows: Query<&CursorOptions, With<Window>>,
    player: Query<&Transform, With<Player>>,
    mut cooldown: ResMut<SwingCooldown>,
    mut inventory: ResMut<Inventory>,
    catalog: Res<ItemCatalog>,
    mut queue: Option<ResMut<ArcheryTestQueue>>,
    mut seq: ResMut<ArrowSeq>,
    speed_override: Res<ArcherySpeedOverride>,
    dialogue_open: Option<Res<DialogueOpen>>,
    inventory_open: Option<Res<InventoryOpen>>,
    pause_open: Option<Res<PauseOpen>>,
) {
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
    let touch_attack = touch
        .as_deref()
        .map(|t| t.attack_right_pressed || t.attack_left_pressed)
        .unwrap_or(false);
    let queued = queue
        .as_deref()
        .map(|q| q.shots_remaining > 0)
        .unwrap_or(false);
    if !mouse_attack && !touch_attack && !queued {
        return;
    }
    if cooldown.remaining > 0.0 {
        return;
    }
    // A bow must be equipped — otherwise this press belongs to
    // melee (or to fists, when nothing is equipped).
    let Some(bow_def) = inventory
        .equipped_weapon
        .and_then(|id| catalog.defs.get(&id))
        .filter(|def| def.kind == ItemKind::Weapon && def.is_bow)
    else {
        return;
    };
    let bow_damage = bow_def.damage;
    let bow_speed = bow_def.speed;
    let bow_name = bow_def.name.clone();
    // The arrow: the first Ammo stack in inventory order (no
    // equipped-ammo slot exists yet — the slot is a follow-up).
    let Some(ammo_stack) = inventory
        .stacks
        .iter()
        .find(|s| {
            s.count > 0
                && catalog
                    .defs
                    .get(&s.base_form_id)
                    .map(|def| def.kind == ItemKind::Ammo)
                    .unwrap_or(false)
        })
        .map(|s| s.base_form_id)
    else {
        crate::startup_log::log("cannot fire: no arrows");
        return;
    };
    let ammo_def = &catalog.defs[&ammo_stack];
    let Some(model_path) = ammo_def.projectile_model.clone() else {
        crate::startup_log::log(&format!(
            "cannot fire {}: no flight model (PROJ record missing)",
            ammo_def.name
        ));
        return;
    };
    let speed = speed_override.0.unwrap_or(ammo_def.projectile_speed);
    if speed <= 0.0 {
        crate::startup_log::log(&format!(
            "cannot fire {}: no flight speed (PROJ record missing)",
            ammo_def.name
        ));
        return;
    }
    let Ok(transform) = player.single() else {
        return;
    };
    // The shot happens (cooldown spent, arrow consumed, queue
    // consumed) even when it hits nothing — a miss is still a
    // shot.
    cooldown.remaining = swing_cooldown_secs(bow_speed);
    if !inventory.remove(ammo_stack, 1) {
        return;
    }
    if let Some(q) = queue.as_deref_mut() {
        if q.shots_remaining > 0 {
            q.shots_remaining -= 1;
        }
    }
    let damage = arrow_damage(bow_damage, ammo_def.damage);
    let ammo_name = ammo_def.name.clone();
    let gravity = arrow_gravity_mps2(ammo_def.projectile_gravity);
    let forward = *transform.forward();
    let origin = transform.translation + forward * ARROW_SPAWN_AHEAD_M;
    let velocity = forward * speed;
    let arrow_seq = seq.next;
    seq.next += 1;
    commands.spawn((
        Arrow {
            velocity,
            damage,
            bow_name: bow_name.clone(),
            ammo_name: ammo_name.clone(),
            gravity,
            distance_flown: 0.0,
            stuck: false,
            seq: arrow_seq,
            mesh_entities: Vec::new(),
        },
        Transform {
            translation: origin,
            rotation: arrow_rotation(velocity),
            scale: Vec3::ONE,
        },
        NeedsArrowMesh { model_path },
        SceneEntity,
    ));
    crate::startup_log::log(&format!(
        "Fired {ammo_name} from {bow_name} ({} damage) at {:.1} m/s",
        damage, speed
    ));
}

/// Attach the flight mesh to arrows that still need one: load
/// the PROJ flight NIF through the asset provider, convert it
/// like `drop_item` does, and spawn one FLAT top-level mesh
/// entity per arrow shape (same pattern as `drop_item` and the
/// scene meshes — the engine does not parent meshes), tagged
/// with `ArrowMesh` so `sync_arrow_meshes` follows the arrow.
/// The mesh entity starts at the root's current transform. A
/// failed load removes the marker without retrying — the arrow
/// flies invisibly and the log says so. No asset provider (the
/// headless App test): skip, the marker stays, physics runs.
pub fn spawn_arrow_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut assets: Option<ResMut<crate::assets::AssetProvider>>,
    mut pending: Query<(Entity, &NeedsArrowMesh, &Transform, &mut Arrow)>,
) {
    let Some(assets) = assets.as_deref_mut() else {
        return;
    };
    for (entity, need, root_transform, mut arrow) in &mut pending {
        let nif_path = format!("meshes\\{}", need.model_path);
        let nif = assets
            .read_file(&nif_path)
            .and_then(|bytes| dovah_nif::NifFile::from_bytes(&bytes).ok());
        let Some(nif) = nif else {
            crate::startup_log::log(&format!(
                "arrow mesh missing or unparsable: {nif_path} (arrow flies invisible)"
            ));
            commands.entity(entity).remove::<NeedsArrowMesh>();
            continue;
        };
        let sub_meshes = nif.meshes();
        let shape_list: Vec<(&str, usize)> = sub_meshes
            .iter()
            .map(|m| (m.name.as_str(), m.vertices.len()))
            .collect();
        let shape_idx = select_flight_shapes(&shape_list);
        let mut converted: Vec<(Mesh, Option<String>)> = Vec::new();
        for i in shape_idx {
            let Some(mesh) = crate::convert::nif_mesh_to_bevy(&sub_meshes[i]) else {
                continue;
            };
            converted.push((mesh, sub_meshes[i].textures.diffuse.clone()));
        }
        if converted.is_empty() {
            crate::startup_log::log(&format!(
                "arrow mesh has no geometry: {nif_path} (arrow flies invisible)"
            ));
            commands.entity(entity).remove::<NeedsArrowMesh>();
            continue;
        }
        let mut material_cache: HashMap<Option<String>, Handle<StandardMaterial>> = HashMap::new();
        let root_transform = *root_transform;
        for (mesh, diffuse) in converted {
            let material = crate::player_model::doll_material(
                &mut materials,
                &mut images,
                assets,
                &mut material_cache,
                &diffuse,
            );
            let mesh_entity = commands
                .spawn((
                    ArrowMesh { arrow: entity },
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(material),
                    root_transform,
                    SceneEntity,
                ))
                .id();
            arrow.mesh_entities.push(mesh_entity);
        }
        commands.entity(entity).remove::<NeedsArrowMesh>();
    }
}

/// Keep each arrow's flat mesh entities on the arrow: copy the
/// root's transform onto them every frame (they are top-level
/// entities, so nothing propagates it for them). Runs chained
/// after `arrow_flight`, so the mesh is never a frame behind.
/// A mesh whose arrow is gone (despawned by a hit, the range
/// cap, or the stuck cap — the flight system despawns both,
/// this is the backstop) is despawned with it.
pub fn sync_arrow_meshes(
    mut commands: Commands,
    arrows: Query<&Transform, With<Arrow>>,
    // Without<Arrow>: mesh entities never carry Arrow (they link
    // back via ArrowMesh.arrow), but Bevy's B0001 check needs the
    // disjointness declared — without it the two Transform
    // queries conflict at system init and the app panics on launch.
    mut meshes: Query<(Entity, &ArrowMesh, &mut Transform), Without<Arrow>>,
) {
    for (mesh_entity, link, mut mesh_transform) in &mut meshes {
        match arrows.get(link.arrow) {
            Ok(arrow_transform) => {
                *mesh_transform = *arrow_transform;
            }
            Err(_) => {
                commands.entity(mesh_entity).despawn();
            }
        }
    }
}

// ----------------------------------------------------------------
// The flight system.
// ----------------------------------------------------------------

/// Move every flying arrow and resolve hits. Gravity applies
/// once per frame; the travel is substepped (no substep longer
/// than ARROW_SUBSTEP_M) so a 51.4 m/s arrow cannot tunnel.
/// Per substep, in order: a combatant's body cylinder (damage
/// through the combat path, arrow despawns), the floor/terrain
/// height under the tip, the obstacle grid at the tip (both
/// stick the arrow in place). Past the PROJ range the arrow
/// despawns. Stuck arrows are capped (oldest first).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
pub fn arrow_flight(
    mut commands: Commands,
    time: Res<Time>,
    mut arrows: Query<(Entity, &mut Arrow, &mut Transform)>,
    mut combatants: ResMut<Combatants>,
    mut world: ResMut<WorldChanges>,
    // Without<Arrow>: the arrows query above also takes &mut
    // Transform, so the two queries need disjoint filters or
    // Bevy rejects the system at init (B0001 — the same shape
    // as melee_attack's Without<Player>).
    mut parts: Query<(&NpcPart, &mut Transform), (Without<Player>, Without<Arrow>)>,
    grid: Option<Res<FloorGrid>>,
    obstacles: Option<Res<ObstacleGrid>>,
    terrain: Option<Res<TerrainHeights>>,
) {
    let dt = time.delta_secs();
    for (entity, mut arrow, mut transform) in &mut arrows {
        if arrow.stuck {
            continue;
        }
        // Gravity once per frame (Euler — the substeps below
        // share the frame's velocity).
        arrow.velocity.y -= arrow.gravity * dt;
        let travel = arrow.velocity.length() * dt;
        let steps = flight_substeps(travel);
        let step_dt = dt / steps as f32;
        let mut hit_npc: Option<u32> = None;
        let mut stuck = false;
        for _ in 0..steps {
            transform.translation += arrow.velocity * step_dt;
            arrow.distance_flown += arrow.velocity.length() * step_dt;
            let tip = transform.translation;
            // A combatant's body cylinder first (a hit inside
            // a body beats the wall behind it).
            for (ref_form_id, npc) in combatants.npcs.iter() {
                if npc.dead {
                    continue;
                }
                if hits_combatant(tip, npc.position) {
                    hit_npc = Some(*ref_form_id);
                    break;
                }
            }
            if hit_npc.is_some() {
                break;
            }
            // The ground under the tip (floor grid + terrain,
            // whichever is higher — the player's rule).
            let mut ground = match &grid {
                Some(grid) => grid.ground_height(tip.x, tip.z, tip.y),
                None => f32::NEG_INFINITY,
            };
            if let Some(terrain) = &terrain {
                if let Some(th) = terrain.height_at(tip.x, tip.z) {
                    ground = ground.max(th);
                }
            }
            if tip.y <= ground {
                stuck = true;
                break;
            }
            // A wall or object at the tip (a 10 cm band around
            // it — the tip is a point, not a body).
            if let Some(obstacles) = &obstacles {
                if obstacles.blocked(tip.x, tip.z, tip.y - 0.05, tip.y + 0.05) {
                    stuck = true;
                    break;
                }
            }
            if arrow.distance_flown >= ARROW_MAX_RANGE_M {
                break;
            }
        }
        // Re-aim the mesh at the velocity (gravity curves it).
        transform.rotation = arrow_rotation(arrow.velocity);
        if let Some(target_ref) = hit_npc {
            if let Some(target) = combatants.npcs.get_mut(&target_ref) {
                let (new_health, died) = apply_damage(target.health, arrow.damage);
                target.health = new_health;
                if died {
                    target.dead = true;
                    world.dead_npcs.insert(target_ref);
                    fall_npc_parts(&mut parts, target_ref);
                    crate::startup_log::log(&format!(
                        "Shot {} with {} ({} damage) — killed",
                        target.name, arrow.bow_name, arrow.damage
                    ));
                } else {
                    crate::startup_log::log(&format!(
                        "Shot {} for {} ({} + {}) — HP {}/{}",
                        target.name,
                        arrow.damage,
                        arrow.bow_name,
                        arrow.ammo_name,
                        new_health,
                        target.max_health
                    ));
                }
            }
            despawn_arrow(&mut commands, entity, &mut arrow);
        } else if stuck {
            arrow.stuck = true;
            arrow.velocity = Vec3::ZERO;
            crate::startup_log::log(&format!(
                "Arrow stuck at ({:.1}, {:.1}, {:.1})",
                transform.translation.x, transform.translation.y, transform.translation.z
            ));
        } else if arrow.distance_flown >= ARROW_MAX_RANGE_M {
            despawn_arrow(&mut commands, entity, &mut arrow);
        }
    }
    // The stuck-arrow cap: despawn the oldest stuck arrows
    // past the cap (a long session cannot accumulate them).
    let mut stuck_arrows: Vec<(u32, Entity, Vec<Entity>)> = arrows
        .iter()
        .filter(|(_, arrow, _)| arrow.stuck)
        .map(|(entity, arrow, _)| (arrow.seq, entity, arrow.mesh_entities.clone()))
        .collect();
    if stuck_arrows.len() > MAX_STUCK_ARROWS {
        stuck_arrows.sort_by_key(|(seq, _, _)| *seq);
        let excess = stuck_arrows.len() - MAX_STUCK_ARROWS;
        for (_, entity, mesh_entities) in stuck_arrows.into_iter().take(excess) {
            for mesh_entity in mesh_entities {
                commands.entity(mesh_entity).despawn();
            }
            commands.entity(entity).despawn();
        }
    }
}

/// Despawn an arrow root and its flat mesh entities together
/// (a hit or the range cap — the stuck cap despawns from a
/// collected list instead, since it runs after the loop).
fn despawn_arrow(commands: &mut Commands, entity: Entity, arrow: &mut Arrow) {
    for mesh_entity in std::mem::take(&mut arrow.mesh_entities) {
        commands.entity(mesh_entity).despawn();
    }
    commands.entity(entity).despawn();
}

// ----------------------------------------------------------------
// The `--archery-test` seed.
// ----------------------------------------------------------------

/// Apply the `--archery-test` seed (OnEnter InGame, after
/// `build_combatants`): teleport the player 8 m from the
/// seeded NPC (Orgnar when present, else the nearest
/// combatant) facing its chest, grant a HuntingBow + N
/// IronArrows, equip the bow, and queue the scripted shots.
/// Mirrors `combat::apply_melee_test_seed` (1.2 m, fists).
pub fn apply_archery_test_seed(
    mut commands: Commands,
    seed: Option<Res<ArcheryTestSeed>>,
    combatants: Option<Res<Combatants>>,
    catalog: Res<ItemCatalog>,
    mut inventory: ResMut<Inventory>,
    mut player: Query<(&mut Transform, &mut crate::player::LookAngles), With<Player>>,
) {
    let Some(seed) = seed else {
        return;
    };
    let (Some(combatants), Ok((mut transform, mut angles))) = (combatants, player.single_mut())
    else {
        commands.remove_resource::<ArcheryTestSeed>();
        return;
    };
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
        commands.remove_resource::<ArcheryTestSeed>();
        return;
    };
    let target = &combatants.npcs[&target_ref];
    let target_pos = target.position;
    let target_name = target.name.clone();
    let target_level = target.level;
    let target_health = target.max_health;
    // Approach from the player's side, 8 m out (archery range),
    // eye height, facing the target's chest.
    let mut dir = Vec3::new(feet.x - target_pos.x, 0.0, feet.z - target_pos.z);
    if dir.length_squared() < 0.01 {
        dir = Vec3::X;
    }
    dir = dir.normalize();
    let new_feet = target_pos + dir * ARCHERY_TEST_DISTANCE_M;
    let eye = new_feet + Vec3::Y * EYE_HEIGHT;
    let look_at = target_pos + Vec3::Y * 1.2;
    *transform = Transform::from_translation(eye).looking_at(look_at, Vec3::Y);
    *angles = crate::player::LookAngles::from_forward(look_at - eye);
    // Grant the bow + arrows and equip the bow (the player
    // starts with neither — the seed is the proof's supply).
    inventory.add(SEED_BOW_FORM_ID, 1);
    inventory.add(SEED_ARROW_FORM_ID, seed.shots as i32);
    if let Some(def) = catalog.def(SEED_BOW_FORM_ID) {
        if !inventory.is_equipped(SEED_BOW_FORM_ID) {
            inventory.equip(SEED_BOW_FORM_ID, def);
        }
    }
    commands.insert_resource(ArcheryTestQueue {
        shots_remaining: seed.shots,
    });
    crate::startup_log::log(&format!(
        "archery-test: player at ({:.2}, {:.2}, {:.2}) facing {} (ref 0x{:08x}, level {}, {} HP), HuntingBow + {} IronArrows, {} shots queued",
        eye.x, eye.y, eye.z, target_name, target_ref, target_level, target_health, seed.shots, seed.shots
    ));
    commands.remove_resource::<ArcheryTestSeed>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::{melee_attack, Combatant, Combatants, MeleeTestQueue, SwingCooldown};
    use crate::inventory::{Inventory, ItemCatalog, ItemDef, ItemKind};
    use crate::touch::TouchInputState;

    fn bow_def() -> ItemDef {
        ItemDef {
            base_form_id: SEED_BOW_FORM_ID,
            name: "Hunting Bow".to_string(),
            kind: ItemKind::Weapon,
            value: 50,
            weight: 7.0,
            damage: 7,
            speed: 0.9375,
            reach: 1.0,
            is_bow: true,
            projectile_model: None,
            projectile_speed: 0.0,
            projectile_gravity: 0.0,
            armor_rating: 0.0,
            slot: None,
            worn_model: None,
            description: None,
            effects: Vec::new(),
            ground_model: None,
        }
    }

    fn arrow_def() -> ItemDef {
        ItemDef {
            base_form_id: SEED_ARROW_FORM_ID,
            name: "Iron Arrow".to_string(),
            kind: ItemKind::Ammo,
            value: 1,
            weight: 0.1,
            damage: 8,
            speed: 1.0,
            reach: 1.0,
            is_bow: false,
            projectile_model: Some("Weapons\\Iron\\IronArrowFlight.nif".to_string()),
            projectile_speed: 3600.0 / 70.0,
            projectile_gravity: 0.35,
            armor_rating: 0.0,
            slot: None,
            worn_model: None,
            description: None,
            effects: Vec::new(),
            ground_model: Some("Weapons\\Iron\\IronArrow.nif".to_string()),
        }
    }

    #[test]
    fn arrow_damage_sums_bow_and_ammo() {
        assert_eq!(arrow_damage(7, 8), 15); // HuntingBow + IronArrow
        assert_eq!(arrow_damage(6, 10), 16); // LongBow + SteelArrow
        assert_eq!(arrow_damage(19, 24), 43); // DaedricBow + DaedricArrow
        assert_eq!(arrow_damage(u16::MAX, 1), u16::MAX);
    }

    #[test]
    fn arrow_gravity_scales_with_engine_gravity() {
        assert!((arrow_gravity_mps2(0.35) - 0.35 * GRAVITY).abs() < 1e-6);
        assert_eq!(arrow_gravity_mps2(0.0), 0.0);
    }

    #[test]
    fn flight_substeps_caps_step_length() {
        assert_eq!(flight_substeps(0.0), 1);
        assert_eq!(flight_substeps(0.25), 1);
        assert_eq!(flight_substeps(0.26), 2);
        // One clamped 250 ms frame at full arrow speed.
        assert_eq!(flight_substeps(51.4 * 0.25), 52);
    }

    #[test]
    fn hits_combatant_cylinder() {
        let feet = Vec3::new(0.0, 0.0, 0.0);
        assert!(hits_combatant(Vec3::new(0.0, 1.2, 0.0), feet));
        assert!(hits_combatant(Vec3::new(0.4, 0.1, 0.0), feet));
        assert!(hits_combatant(Vec3::new(0.0, 1.9, 0.0), feet));
        assert!(!hits_combatant(Vec3::new(0.6, 1.2, 0.0), feet));
        assert!(!hits_combatant(Vec3::new(0.0, 2.0, 0.0), feet));
        assert!(!hits_combatant(Vec3::new(0.0, -0.1, 0.0), feet));
        assert!(!hits_combatant(Vec3::new(0.4, 1.2, 0.4), feet));
    }

    #[test]
    fn arrow_rotation_maps_plus_z_to_velocity() {
        // Straight ahead (-Z, the camera's forward).
        let r = arrow_rotation(Vec3::new(0.0, 0.0, -1.0));
        assert!((r * Vec3::Z - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5);
        // Straight up, and a diagonal.
        let r = arrow_rotation(Vec3::Y);
        assert!((r * Vec3::Z - Vec3::Y).length() < 1e-5);
        let dir = Vec3::new(1.0, 0.5, -1.0).normalize();
        let r = arrow_rotation(dir);
        assert!((r * Vec3::Z - dir).length() < 1e-5);
        // Zero velocity is identity, never NaN.
        assert_eq!(arrow_rotation(Vec3::ZERO), Quat::IDENTITY);
    }

    #[test]
    fn select_flight_shapes_filters_trail_and_flow() {
        // The flight NIF: arrow + trail (+ flow on steel).
        let flight = [
            ("trailShort", 30),
            ("Flow:0", 9),
            ("IronArrowFlight:0", 214),
        ];
        assert_eq!(select_flight_shapes(&flight), vec![2]);
        // The AMMO ground model: the exact "Arrow:0" wins over
        // the quiver arrows.
        let ground = [("Arrow1", 59), ("ArrowQuiver:0", 140), ("Arrow:0", 210)];
        assert_eq!(select_flight_shapes(&ground), vec![2]);
        // No arrow-named shape: the largest wins.
        let odd = [("a", 10), ("b", 50)];
        assert_eq!(select_flight_shapes(&odd), vec![1]);
        assert!(select_flight_shapes(&[]).is_empty());
    }

    /// The real fire + flight systems, headless: a bow equipped,
    /// arrows in the inventory, one queued shot, Orgnar 8 m
    /// ahead. The shot fires (arrow consumed, Arrow spawned),
    /// flies, and hits (Orgnar takes 15 damage). No asset
    /// provider exists in the test app, so the mesh system is
    /// not registered — the arrow flies without its mesh, which
    /// is the point of the decoupling. Follows the melee App
    /// test pattern (systems added directly, manual time).
    #[test]
    fn archery_fire_flies_and_hits_through_the_real_system() {
        use bevy::app::App;
        use bevy::prelude::MinimalPlugins;
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // Manual 16 ms steps: the arrow crosses 8 m at 51.4 m/s
        // in ~0.156 s, about 10 updates.
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )));
        app.add_systems(Update, (archery_fire, arrow_flight).chain());
        // The player at the origin, facing Orgnar's chest.
        app.world_mut().spawn((
            Player,
            Transform::from_translation(Vec3::new(0.0, EYE_HEIGHT, 0.0))
                .looking_at(Vec3::new(0.0, 1.2, -8.0), Vec3::Y),
        ));
        // Orgnar 8 m ahead, 75 HP (the vanilla value).
        let mut combatants = Combatants::default();
        combatants.npcs.insert(
            0x0001_3486,
            Combatant {
                base_form_id: 0x0001_3479,
                name: "Orgnar".to_string(),
                position: Vec3::new(0.0, 0.0, -8.0),
                level: 4,
                max_health: 75,
                health: 75,
                dead: false,
            },
        );
        app.insert_resource(combatants);
        app.insert_resource(SwingCooldown::default());
        app.insert_resource(WorldChanges::default());
        app.insert_resource(ArrowSeq::default());
        app.insert_resource(ArcherySpeedOverride::default());
        let mut catalog = ItemCatalog::default();
        catalog.defs.insert(SEED_BOW_FORM_ID, bow_def());
        catalog.defs.insert(SEED_ARROW_FORM_ID, arrow_def());
        app.insert_resource(catalog);
        let mut inventory = Inventory::default();
        inventory.add(SEED_BOW_FORM_ID, 1);
        inventory.add(SEED_ARROW_FORM_ID, 3);
        inventory.equipped_weapon = Some(SEED_BOW_FORM_ID);
        app.insert_resource(inventory);
        app.insert_resource(ArcheryTestQueue { shots_remaining: 1 });
        // The first update reports delta 0 (no previous
        // instant): the shot fires, the arrow does not move.
        // ~10 more updates carry it 8 m; 60 gives margin.
        for _ in 0..60 {
            app.update();
            let combatants = app.world().resource::<Combatants>();
            if combatants.npcs[&0x0001_3486].health < 75 {
                break;
            }
        }
        let combatants = app.world().resource::<Combatants>();
        assert_eq!(combatants.npcs[&0x0001_3486].health, 60); // 75 - 15
        let inventory = app.world().resource::<Inventory>();
        assert_eq!(inventory.count(SEED_ARROW_FORM_ID), 2);
        let queue = app.world().resource::<ArcheryTestQueue>();
        assert_eq!(queue.shots_remaining, 0);
    }

    /// The phone path: a touch ATTACK press (the flag
    /// `track_touches` sets from the ATTACK buttons) fires the
    /// bow through the real fire system — no queue, no mouse.
    /// This is the path the desktop suite could not see before
    /// v0.4.12 (the touch crash): the flag is the phone's only
    /// trigger, so it gets its own test. The flag is a
    /// one-frame pulse — it is cleared after the first update,
    /// as the next `track_touches` would.
    #[test]
    fn touch_attack_flag_fires_the_bow_through_the_real_system() {
        use bevy::app::App;
        use bevy::prelude::MinimalPlugins;
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )));
        app.add_systems(Update, (archery_fire, arrow_flight).chain());
        app.world_mut().spawn((
            Player,
            Transform::from_translation(Vec3::new(0.0, EYE_HEIGHT, 0.0))
                .looking_at(Vec3::new(0.0, 1.2, -8.0), Vec3::Y),
        ));
        let mut combatants = Combatants::default();
        combatants.npcs.insert(
            0x0001_3486,
            Combatant {
                base_form_id: 0x0001_3479,
                name: "Orgnar".to_string(),
                position: Vec3::new(0.0, 0.0, -8.0),
                level: 4,
                max_health: 75,
                health: 75,
                dead: false,
            },
        );
        app.insert_resource(combatants);
        app.insert_resource(SwingCooldown::default());
        app.insert_resource(WorldChanges::default());
        app.insert_resource(ArrowSeq::default());
        app.insert_resource(ArcherySpeedOverride::default());
        let mut catalog = ItemCatalog::default();
        catalog.defs.insert(SEED_BOW_FORM_ID, bow_def());
        catalog.defs.insert(SEED_ARROW_FORM_ID, arrow_def());
        app.insert_resource(catalog);
        let mut inventory = Inventory::default();
        inventory.add(SEED_BOW_FORM_ID, 1);
        inventory.add(SEED_ARROW_FORM_ID, 3);
        inventory.equipped_weapon = Some(SEED_BOW_FORM_ID);
        app.insert_resource(inventory);
        let mut touch = TouchInputState::default();
        touch.attack_right_pressed = true;
        app.insert_resource(touch);
        app.update();
        app.world_mut()
            .resource_mut::<TouchInputState>()
            .attack_right_pressed = false;
        for _ in 0..60 {
            app.update();
            let combatants = app.world().resource::<Combatants>();
            if combatants.npcs[&0x0001_3486].health < 75 {
                break;
            }
        }
        let combatants = app.world().resource::<Combatants>();
        assert_eq!(combatants.npcs[&0x0001_3486].health, 60); // 75 - 15
        let inventory = app.world().resource::<Inventory>();
        assert_eq!(inventory.count(SEED_ARROW_FORM_ID), 2);
    }

    /// The same touch press with no bow equipped fires nothing:
    /// the press belongs to melee (fists), not archery — no
    /// cooldown spent, no arrow consumed.
    #[test]
    fn touch_attack_flag_fires_nothing_without_a_bow() {
        use bevy::app::App;
        use bevy::prelude::MinimalPlugins;
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )));
        app.add_systems(Update, archery_fire);
        app.world_mut().spawn((
            Player,
            Transform::from_translation(Vec3::new(0.0, EYE_HEIGHT, 0.0))
                .looking_at(Vec3::new(0.0, 1.2, -8.0), Vec3::Y),
        ));
        app.insert_resource(SwingCooldown::default());
        app.insert_resource(ArrowSeq::default());
        app.insert_resource(ArcherySpeedOverride::default());
        let mut catalog = ItemCatalog::default();
        catalog.defs.insert(SEED_BOW_FORM_ID, bow_def());
        catalog.defs.insert(SEED_ARROW_FORM_ID, arrow_def());
        app.insert_resource(catalog);
        let mut inventory = Inventory::default();
        inventory.add(SEED_BOW_FORM_ID, 1);
        inventory.add(SEED_ARROW_FORM_ID, 3);
        // Fists: nothing equipped.
        inventory.equipped_weapon = None;
        app.insert_resource(inventory);
        let mut touch = TouchInputState::default();
        touch.attack_right_pressed = true;
        app.insert_resource(touch);
        for _ in 0..10 {
            app.update();
        }
        let cooldown = app.world().resource::<SwingCooldown>();
        assert_eq!(cooldown.remaining, 0.0);
        let inventory = app.world().resource::<Inventory>();
        assert_eq!(inventory.count(SEED_ARROW_FORM_ID), 3);
    }

    /// Melee yields to bows: a queued melee attack with a bow
    /// equipped does nothing (no damage, no cooldown spent,
    /// queue untouched) — the press belongs to archery.
    #[test]
    fn melee_attack_yields_to_an_equipped_bow() {
        use bevy::app::App;
        use bevy::prelude::MinimalPlugins;
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )));
        app.add_systems(Update, melee_attack);
        app.world_mut().spawn((
            Player,
            Transform::from_translation(Vec3::new(0.0, EYE_HEIGHT, 0.0))
                .looking_at(Vec3::new(0.0, 1.2, -1.2), Vec3::Y),
        ));
        let mut combatants = Combatants::default();
        combatants.npcs.insert(
            0x0001_3486,
            Combatant {
                base_form_id: 0x0001_3479,
                name: "Orgnar".to_string(),
                position: Vec3::new(0.0, 0.0, -1.2),
                level: 4,
                max_health: 75,
                health: 75,
                dead: false,
            },
        );
        app.insert_resource(combatants);
        app.insert_resource(SwingCooldown::default());
        app.insert_resource(WorldChanges::default());
        let mut catalog = ItemCatalog::default();
        catalog.defs.insert(SEED_BOW_FORM_ID, bow_def());
        app.insert_resource(catalog);
        let mut inventory = Inventory::default();
        inventory.add(SEED_BOW_FORM_ID, 1);
        inventory.equipped_weapon = Some(SEED_BOW_FORM_ID);
        app.insert_resource(inventory);
        // A queued MELEE attack (not an archery shot).
        app.insert_resource(MeleeTestQueue {
            attacks_remaining: 1,
        });
        for _ in 0..10 {
            app.update();
        }
        let combatants = app.world().resource::<Combatants>();
        assert_eq!(combatants.npcs[&0x0001_3486].health, 75);
        let cooldown = app.world().resource::<SwingCooldown>();
        assert_eq!(cooldown.remaining, 0.0);
        // The melee queue is untouched — the guard returns
        // before the queue is consumed.
        let queue = app.world().resource::<MeleeTestQueue>();
        assert_eq!(queue.attacks_remaining, 1);
    }

    /// The seed teleports the player 8 m from Orgnar, facing
    /// him, grants a HuntingBow + N IronArrows, equips the
    /// bow, and queues the shots.
    #[test]
    fn archery_test_seed_teleports_and_equips() {
        use bevy::app::App;

        let mut app = App::new();
        app.add_systems(bevy::app::Startup, apply_archery_test_seed);
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
        app.insert_resource(ArcheryTestSeed {
            shots: 3,
            npc_base: Some(0x0001_3479),
        });
        let mut catalog = ItemCatalog::default();
        catalog.defs.insert(SEED_BOW_FORM_ID, bow_def());
        catalog.defs.insert(SEED_ARROW_FORM_ID, arrow_def());
        app.insert_resource(catalog);
        app.insert_resource(Inventory::default());
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
        // 8 m from Orgnar (archery range).
        assert!((feet.distance(target) - ARCHERY_TEST_DISTANCE_M).abs() < 0.01);
        // Facing him: the forward dot with the direction to
        // the target is ~1.
        let to_target = (target - feet).normalize();
        let forward = *transform.forward();
        assert!(forward.dot(to_target) > 0.99);
        // Bow equipped, 3 arrows in the inventory, 3 shots
        // queued.
        let inventory = app.world().resource::<Inventory>();
        assert_eq!(inventory.equipped_weapon, Some(SEED_BOW_FORM_ID));
        assert_eq!(inventory.count(SEED_ARROW_FORM_ID), 3);
        let queue = app.world().resource::<ArcheryTestQueue>();
        assert_eq!(queue.shots_remaining, 3);
    }
}
