//! M15: swinging doors.
//!
//! Doors in Skyrim swing open on USE. The panel is NOT centred on
//! the ref origin by accident: every door NIF hangs its panel
//! under a hinge node (`Door`, `Door01`, `Door02`, ...) whose
//! origin is the panel's edge, and carries its own open/close
//! animation as `NiTransformData` (XYZ Euler keys on the hinge
//! node's Z axis). The engine reproduces both from the data:
//!
//! * the hinge is the parent node's composed translation
//!   (`Mesh::parent_transform`, dovah-nif), never guessed;
//! * the open angle and swing duration come from the NIF's Open
//!   sequence — the `NiTransformData` whose Z keys start at the
//!   rest angle (0, or -2π — FarmhouseAnimDoor02 authors its rest
//!   state as -2π) and end elsewhere; the angle is the keys'
//!   DELTA. Load doors crack 18 deg over 0.6 s
//!   (FarmhouseLDoor01: the player never walks through one in the
//!   real game, the load screen takes over); walk-through doors
//!   swing ~92 deg over 1.0 s (FarmhouseAnimDoor01, with a slight
//!   overshoot the keys carry; AnimDoor02 swings the other way,
//!   +92 deg, from its opposite hinge).
//!
//! Sign convention: a NIF rotation about the hinge node's local Z
//! by angle `a` is a Bevy rotation about Y by the SAME angle `a`
//! (the axis map is a proper rotation — conjugation preserves the
//! angle). This is NOT the ESM yaw rule: `to_bevy_rotation`
//! negates the Z angle because the ESM stores clockwise-positive
//! yaw, while NIF-internal rotations are standard
//! counterclockwise-positive. Proven numerically: the NIF's -18
//! deg moves the panel's free edge toward Skyrim +Y, and Bevy
//! `R_y(-18 deg)` moves it toward Bevy -Z, which IS Skyrim +Y.
//!
//! Double doors (secret door): Delphine's secret door hangs its
//! two panels under `LeftDoor01`/`RightDoor01` and carries TWO
//! opposite Open sequences (±101.2 deg). Which panel gets which
//! angle comes from the NIF's `NiControllerSequence` linkage
//! (dovah-nif `open_angles_by_node`) — the game's own answer —
//! NOT from geometry: the geometric invariant ("free edge moves
//! toward local +Y") is WRONG for this door, whose panels both
//! swing toward -Y (into the room). Each panel carries an
//! `angle_scale` (its own open angle / the door's open angle);
//! the door animates one angle and each panel swings
//! `angle * angle_scale`, so both panels reach their own open
//! angles together. Single-panel doors get scale 1.0 and behave
//! exactly as before.
//!
//! A closed door blocks passage: movement that would cross the
//! doorway plane is cancelled, and the teleport does not fire. An
//! open door lets the player through, and the teleport fires on
//! crossing the plane (M15 replaces the M9 proximity edge for
//! swinging doors — proximity + an open-gate would teleport the
//! player the moment they pressed USE while standing inside the
//! 1.2 m radius, before walking through). Non-swinging XTEL doors
//! keep the old proximity behavior. The 30-frame arrival grace
//! still applies to both.

use crate::scene::Player;
use crate::touch::TouchInputState;
use bevy::ecs::prelude::*;
use bevy::input::keyboard::KeyCode;
use bevy::input::ButtonInput;
use bevy::math::Vec3;
use bevy::time::Time;
use bevy::transform::components::Transform;
use std::collections::{HashMap, HashSet};

/// How close the player must be (feet to door ref, horizontal) to
/// USE a door.
pub const USE_RANGE_M: f32 = 2.5;
/// Lateral slack around the doorway when testing a plane crossing.
const DOORWAY_MARGIN_M: f32 = 0.3;
/// Doorway height above the door ref's base, in metres.
const DOORWAY_HEIGHT_M: f32 = 2.6;
/// How far BELOW the door ref's base a crossing still counts, in
/// metres. Matches the M9 proximity edge's vertical tolerance
/// (DOOR_TRIGGER_HEIGHT_M in lib.rs): a player who reaches a
/// doorway from lower ground — the inn porch is 1.35 m above the
/// street — must still cross into it. The old 0.1 m floor made
/// the inn return impossible from street level even with the
/// door open (the crossing window's floor sat at base_y - 0.1,
/// the player's feet at base_y - 1.35). False-fire risk (walking
/// under a doorway) is the same as the proximity edge already
/// accepts, and the door must still be open enough + the player
/// must cross the plane within the doorway width.
const DOORWAY_BELOW_M: f32 = 2.0;
/// Fallback open angle (radians) when a door NIF carries no
/// parseable animation: 90 deg, the walk-through-door swing, with
/// the negative sign both real door NIFs use.
const FALLBACK_OPEN_ANGLE_RAD: f32 = -std::f32::consts::FRAC_PI_2;
/// Fallback swing duration in seconds.
const FALLBACK_DURATION_S: f32 = 1.0;

/// A door model's open animation, read from its own NIF.
#[derive(Debug, Clone, Copy)]
pub struct DoorAnim {
    /// Open angle in radians (Bevy sign = NIF sign — see the
    /// module doc). Negative for both real door NIFs.
    pub open_angle: f32,
    /// Seconds for the full swing.
    pub duration: f32,
}

impl DoorAnim {
    /// The fallback when a door model has no parseable animation.
    pub fn fallback() -> Self {
        DoorAnim {
            open_angle: FALLBACK_OPEN_ANGLE_RAD,
            duration: FALLBACK_DURATION_S,
        }
    }
}

/// What `spawn_nif` reports back about a model's swinging parts
/// (M15): at least one sub-mesh hangs under a hinge node, and the
/// model's open animation (from its NIF, or the fallback).
#[derive(Debug, Clone, Copy)]
pub struct NifSwing {
    pub anim: DoorAnim,
}

/// True when a node name is a door hinge node: exactly `Door`,
/// `LeftDoor` or `RightDoor`, or one of those followed only by
/// digits (`Door01`, `LeftDoor01`, `RightDoor01`). A plain prefix
/// check would also match `DoorBlack` — the static backing
/// shape's name, which must never swing. `LeftDoor`/`RightDoor`
/// are Skyrim's double-door naming convention (the secret door);
/// the hinge sweep over the inn + Riverwood 3x3 (1,834 refs)
/// found the extended rule catches only DOOR bases there.
pub fn is_hinge_node(name: &str) -> bool {
    for base in ["Door", "LeftDoor", "RightDoor"] {
        if name == base {
            return true;
        }
        if name.len() > base.len()
            && name.starts_with(base)
            && name[base.len()..].chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

/// A panel's swing scale from its own open angle (from the
/// NIF's Open sequence, per node) and the door's open angle
/// (the door-level anim): `node_angle / door_angle`. The door
/// animates one angle; the panel swings `angle * scale`, so each
/// panel reaches its own open angle together with the others.
/// `None` (no sequence entry for this panel) or a ~zero door
/// angle means 1.0 — the panel swings with the door, the M15
/// behavior. Pure — unit-tested.
pub fn panel_angle_scale(node_angle: Option<f32>, door_angle: f32) -> f32 {
    let Some(node) = node_angle else {
        return 1.0;
    };
    if door_angle.abs() < 1e-6 {
        return 1.0;
    }
    node / door_angle
}

/// The open angle + duration of a door NIF's Open sequence: the
/// `NiTransformData` whose Z keys start at the door's REST angle
/// and end elsewhere. The rest angle is 0 — but some NIFs author
/// it as -2π (FarmhouseAnimDoor02's Open runs -6.2832 -> -4.6775,
/// a +92 deg swing from the opposite hinge), so the rest test is
/// modulo 2π. The open angle is the DELTA (last - first), not the
/// end value: for a 0-based sequence they are the same, for a
/// -2π-based one the delta is the true swing. The Close sequence
/// starts away from rest and ends at it, so the two never
/// confuse. When several blocks qualify (they should not for
/// doors), the widest swing wins. `None` when the NIF carries no
/// parseable Open sequence.
pub fn open_angle_from_nif(nif: &dovah_nif::NifFile) -> Option<(f32, f32)> {
    // At rest when the angle is 0 modulo 2π (0 or -2π alike).
    let at_rest = |v: f32| {
        let wrapped = v.rem_euclid(2.0 * std::f32::consts::PI);
        wrapped < 1e-3 || wrapped > 2.0 * std::f32::consts::PI - 1e-3
    };
    let mut best: Option<(f32, f32)> = None;
    for data in nif.transform_data() {
        let Some(first) = data.z_keys.first() else {
            continue;
        };
        let Some(last) = data.z_keys.last() else {
            continue;
        };
        if !at_rest(first.value) || at_rest(last.value) {
            continue;
        }
        let candidate = (last.value - first.value, last.time);
        let better = match best {
            None => true,
            Some((angle, _)) => candidate.0.abs() > angle.abs(),
        };
        if better {
            best = Some(candidate);
        }
    }
    best
}

/// A swinging door panel entity: which door it belongs to, the
/// hinge pivot in world space, and the panel's transform with the
/// door closed. The hinge and closed transform live on the
/// component (not the door state) so a double door — two panels,
/// two hinges, one door state — works with no special case.
#[derive(Component, Debug, Clone)]
pub struct DoorPanel {
    pub ref_form_id: u32,
    /// Hinge pivot in Bevy world space (metres).
    pub hinge: Vec3,
    /// Panel transform with the door closed (= the ref transform).
    pub closed: Transform,
    /// Swing scale (secret door): this panel's open angle /
    /// the door's open angle. 1.0 swings with the door; -1.0
    /// swings the opposite way (the right panel of a double
    /// door). From the NIF's Open sequence per-node angles.
    pub angle_scale: f32,
}

/// One door's runtime state, in the [`Doors`] resource. Built by
/// the chunked scene build (`spawn_chunk`) from the ref + the
/// model's swing info; the panel entities carry their own hinges
/// (see [`DoorPanel`]).
#[derive(Debug, Clone)]
pub struct DoorRuntime {
    pub ref_form_id: u32,
    /// The model's open animation.
    pub anim: DoorAnim,
    /// Doorway width in metres (OBND X extent, or 1.4 m).
    pub width: f32,
    /// A point on the doorway plane (the ref position, Bevy).
    pub plane_point: Vec3,
    /// Unit normal of the doorway plane (the ref's facing).
    pub plane_normal: Vec3,
    /// Unit vector along the doorway's width.
    pub lateral: Vec3,
    /// Doorway floor height (the ref's Bevy Y).
    pub base_y: f32,
    /// Whether this door teleports (has an XTEL). Room doors do
    /// not — they swing, and the player walks through.
    pub has_xtel: bool,
    /// Current swing angle (radians). 0 = closed.
    pub angle: f32,
    /// Angle the swing is heading for (0 or `anim.open_angle`).
    pub target_angle: f32,
}

impl DoorRuntime {
    /// Build a door's runtime state from its ref transform
    /// (Bevy), doorway width (metres), XTEL flag, swing info,
    /// and start state. The doorway plane's normal is the ref's
    /// local +Y (Skyrim) — Bevy -Z before the ref rotation — and
    /// the lateral axis is local +X (Bevy +X).
    pub fn new(
        ref_form_id: u32,
        transform: &Transform,
        width: f32,
        has_xtel: bool,
        anim: DoorAnim,
        starts_open: bool,
    ) -> Self {
        let angle = if starts_open { anim.open_angle } else { 0.0 };
        DoorRuntime {
            ref_form_id,
            anim,
            width,
            plane_point: transform.translation,
            plane_normal: transform.rotation * Vec3::NEG_Z,
            lateral: transform.rotation * Vec3::X,
            base_y: transform.translation.y,
            has_xtel,
            angle,
            target_angle: angle,
        }
    }

    /// Open enough to walk through and to teleport: at least half
    /// the open swing. At half swing a load door's free edge has
    /// moved ~0.21 m (the crack reads as open, and the teleport
    /// fires on crossing); a walk-through door at 46 deg leaves a
    /// ~1 m gap.
    pub fn is_open_enough(&self) -> bool {
        self.angle.abs() >= self.anim.open_angle.abs() / 2.0
    }
}

/// All swinging doors in the current scene, keyed by ref form ID.
/// Rebuilt by the chunked scene build on every scene load; the
/// open/closed memory lives separately in [`DoorMemory`] so it
/// survives transitions.
#[derive(Resource, Default, Debug)]
pub struct Doors {
    pub doors: HashMap<u32, DoorRuntime>,
}

impl Doors {
    pub fn get(&self, ref_form_id: u32) -> Option<&DoorRuntime> {
        self.doors.get(&ref_form_id)
    }
}

/// Which doors the player left open, by ref form ID. Survives
/// scene transitions (it is inserted once at app build, never by
/// the scene build): a door left open stays open when the player
/// comes back. Each side of a load door is its own ref with its
/// own state — opening the outside door does not open the inside
/// one.
#[derive(Resource, Default, Debug)]
pub struct DoorMemory {
    pub open: HashSet<u32>,
}

/// Swing a panel's transform about its hinge: `T(hinge) *
/// R_y(angle) * T(-hinge) * closed`. Pure — unit-tested.
pub fn swing_transform(closed: &Transform, hinge: Vec3, angle: f32) -> Transform {
    let rot = bevy::math::Quat::from_rotation_y(angle);
    Transform {
        translation: hinge + rot * (closed.translation - hinge),
        rotation: rot * closed.rotation,
        scale: closed.scale,
    }
}

/// True when the segment `prev -> now` crosses the doorway plane
/// inside the doorway rectangle (within the width + margin
/// laterally, within the doorway height vertically). Pure —
/// unit-tested. Both positions are feet positions in Bevy metres.
pub fn doorway_crossing(door: &DoorRuntime, prev: Vec3, now: Vec3) -> bool {
    let d_prev = (prev - door.plane_point).dot(door.plane_normal);
    let d_now = (now - door.plane_point).dot(door.plane_normal);
    // Opposite signs = crossed. On the plane exactly (0.0) is not
    // a crossing — the next frame will show a side.
    if d_prev == 0.0 || d_now == 0.0 || d_prev.signum() == d_now.signum() {
        return false;
    }
    // Crossing point, interpolated along the segment.
    let t = d_prev / (d_prev - d_now);
    let point = prev + (now - prev) * t;
    let lateral = (point - door.plane_point).dot(door.lateral).abs();
    if lateral > door.width / 2.0 + DOORWAY_MARGIN_M {
        return false;
    }
    if point.y < door.base_y - DOORWAY_BELOW_M || point.y > door.base_y + DOORWAY_HEIGHT_M {
        return false;
    }
    true
}

/// True when the move `current -> next` would carry the player
/// through a door that is still closed (not open enough). Pure —
/// unit-tested.
pub fn door_blocks_move(door: &DoorRuntime, current: Vec3, next: Vec3) -> bool {
    !door.is_open_enough() && doorway_crossing(door, current, next)
}

/// Pure teleport decision for a swinging door (M15): the
/// transition fires iff the door has an XTEL, is open enough to
/// walk through, and the player's feet crossed its doorway plane
/// this frame (`prev_feet` -> `feet`). A closed door never fires
/// — even mid-crossing — and an open door never fires while the
/// player stands still inside the trigger (no `prev_feet`, or no
/// crossing). The arrival grace is applied by the caller, not
/// here. Unit-tested.
pub fn swinging_door_fires(door: &DoorRuntime, prev_feet: Option<Vec3>, feet: Vec3) -> bool {
    if !door.has_xtel || !door.is_open_enough() {
        return false;
    }
    let Some(prev) = prev_feet else {
        return false;
    };
    doorway_crossing(door, prev, feet)
}

/// Nearest door within USE range of the player's feet (horizontal
/// distance to the door ref), or `None`. Pure — unit-tested.
pub fn nearest_door_in_range(doors: &Doors, feet: Vec3) -> Option<u32> {
    let mut best: Option<(u32, f32)> = None;
    for (ref_form_id, door) in &doors.doors {
        let dx = feet.x - door.plane_point.x;
        let dz = feet.z - door.plane_point.z;
        let dist = (dx * dx + dz * dz).sqrt();
        if dist > USE_RANGE_M {
            continue;
        }
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

/// USE (E on keyboard, the USE touch button): toggle the nearest
/// door in range open/closed. The target flips immediately; the
/// swing animates in `door_swing`. The memory records the intent
/// so the door stays as left across transitions. Gated on every
/// menu (UI milestone 2 fixes the M17 wart: `door_use` used to
/// fire with the inventory, dialogue, pause screen, or character
/// menu open — E/USE belongs to the menu then, not the door).
#[allow(clippy::too_many_arguments)]
pub(crate) fn door_use(
    keyboard: Res<ButtonInput<KeyCode>>,
    touch: Option<Res<TouchInputState>>,
    player: Query<&Transform, With<Player>>,
    mut doors: Option<ResMut<Doors>>,
    mut memory: ResMut<DoorMemory>,
    inventory_open: Option<Res<crate::inventory::InventoryOpen>>,
    dialogue_open: Option<Res<crate::dialogue::DialogueOpen>>,
    pause_open: Option<Res<crate::pause::PauseOpen>>,
    character_open: Option<Res<crate::character::CharacterMenuOpen>>,
) {
    let use_pressed = keyboard.just_pressed(KeyCode::KeyE)
        || touch.as_deref().map(|t| t.use_pressed).unwrap_or(false);
    if !use_pressed {
        return;
    }
    if inventory_open.map(|o| o.0).unwrap_or(false)
        || dialogue_open.map(|o| o.0).unwrap_or(false)
        || pause_open.map(|o| o.0).unwrap_or(false)
        || character_open.map(|o| o.0).unwrap_or(false)
    {
        return;
    }
    let Some(doors) = doors.as_deref_mut() else {
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
    let Some(ref_form_id) = nearest_door_in_range(doors, feet) else {
        return;
    };
    let Some(door) = doors.doors.get_mut(&ref_form_id) else {
        return;
    };
    let opening = door.target_angle == 0.0;
    door.target_angle = if opening { door.anim.open_angle } else { 0.0 };
    if opening {
        memory.open.insert(ref_form_id);
    } else {
        memory.open.remove(&ref_form_id);
    }
    crate::startup_log::log(&format!(
        "door {:#010x} {}",
        ref_form_id,
        if opening { "opening" } else { "closing" }
    ));
}

/// Animate swinging doors toward their targets at constant
/// angular speed (the NIF's keys are quadratic — a smooth ease —
/// but linear reads fine at 0.6-1.0 s and keeps the math exact:
/// the panel lands exactly on the closed/open angle). Panel
/// transforms update from their stored closed transform + hinge,
/// so a mid-swing reversal never accumulates error.
pub fn door_swing(
    time: Res<Time>,
    mut doors: Option<ResMut<Doors>>,
    mut panels: Query<(&mut Transform, &DoorPanel)>,
) {
    let Some(doors) = doors.as_deref_mut() else {
        return;
    };
    let dt = time.delta_secs();
    for door in doors.doors.values_mut() {
        if (door.angle - door.target_angle).abs() < 1e-6 {
            continue;
        }
        let speed = door.anim.open_angle.abs() / door.anim.duration.max(0.05);
        let step = speed * dt;
        let diff = door.target_angle - door.angle;
        door.angle = if diff.abs() <= step {
            door.target_angle
        } else {
            door.angle + diff.signum() * step
        };
    }
    for (mut transform, panel) in panels.iter_mut() {
        let Some(door) = doors.doors.get(&panel.ref_form_id) else {
            continue;
        };
        *transform = swing_transform(&panel.closed, panel.hinge, door.angle * panel.angle_scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_door() -> DoorRuntime {
        // Door at the origin, facing +Z (plane normal +Z),
        // doorway along X, 1.37 m wide, base at y = 0.
        DoorRuntime {
            ref_form_id: 0x13419,
            anim: DoorAnim {
                open_angle: -0.31415927,
                duration: 0.6,
            },
            width: 1.37,
            plane_point: Vec3::ZERO,
            plane_normal: Vec3::Z,
            lateral: Vec3::X,
            base_y: 0.0,
            has_xtel: true,
            angle: 0.0,
            target_angle: 0.0,
        }
    }

    #[test]
    fn hinge_node_names() {
        assert!(is_hinge_node("Door"));
        assert!(is_hinge_node("Door01"));
        assert!(is_hinge_node("Door02"));
        assert!(is_hinge_node("LeftDoor"));
        assert!(is_hinge_node("LeftDoor01"));
        assert!(is_hinge_node("RightDoor"));
        assert!(is_hinge_node("RightDoor01"));
        assert!(!is_hinge_node("DoorBlack"));
        assert!(!is_hinge_node("LeftDoorBlack"));
        assert!(!is_hinge_node("RightDoorA"));
        assert!(!is_hinge_node("FarmhouseLDoor01"));
        assert!(!is_hinge_node(""));
        assert!(!is_hinge_node("DoorA"));
    }

    #[test]
    fn panel_angle_scale_secret_door() {
        // The secret door's door-level anim is -101.2 deg (the
        // first widest Open sequence). The linkage gives the left
        // panel -101.2 deg (scale +1.0, swings with the door)
        // and the right panel +101.2 deg (scale -1.0, swings the
        // opposite way).
        let door_angle = -1.7671;
        assert!((panel_angle_scale(Some(-1.7671), door_angle) - 1.0).abs() < 1e-6);
        assert!((panel_angle_scale(Some(1.7671), door_angle) - (-1.0)).abs() < 1e-6);
        // No sequence entry for this panel: swings with the door.
        assert_eq!(panel_angle_scale(None, door_angle), 1.0);
        // Degenerate door angle: no division, swings with the door.
        assert_eq!(panel_angle_scale(Some(1.7671), 0.0), 1.0);
        // A single-panel door: its node angle IS the door angle.
        assert!((panel_angle_scale(Some(-0.31415927), -0.31415927) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn double_door_panels_swing_opposite_into_the_room() {
        // Secret door geometry (Skyrim units -> Bevy metres):
        // left hinge (-39.8, -33.0, 101.9), right hinge
        // (39.8, -33.0, 101.9), both panels' free edges at x = 0
        // (the middle). The door animates to -101.2 deg; the
        // left panel takes scale +1.0, the right -1.0. Both free
        // edges must move toward Bevy +Z (Skyrim -Y, INTO the
        // room — the linkage's answer, not the +Y invariant's)
        // and fold back past their hinges, leaving the doorway
        // open instead of crossing through each other.
        let door_angle = -1.7671;
        let left_scale = panel_angle_scale(Some(-1.7671), door_angle);
        let right_scale = panel_angle_scale(Some(1.7671), door_angle);
        let closed = Transform::IDENTITY;
        let left_hinge = Vec3::new(-39.8 / 70.0, 101.9 / 70.0, 33.0 / 70.0);
        let right_hinge = Vec3::new(39.8 / 70.0, 101.9 / 70.0, 33.0 / 70.0);
        let free_edge = Vec3::new(0.0, 101.9 / 70.0, 33.0 / 70.0);
        let left_swung = swing_transform(&closed, left_hinge, door_angle * left_scale);
        let right_swung = swing_transform(&closed, right_hinge, door_angle * right_scale);
        let left_moved = left_swung.transform_point(free_edge);
        let right_moved = right_swung.transform_point(free_edge);
        // Both free edges moved into the room (+Z) by ~0.56 m.
        assert!(left_moved.z > free_edge.z + 0.5, "left {left_moved:?}");
        assert!(right_moved.z > free_edge.z + 0.5, "right {right_moved:?}");
        // And folded back past their own hinges, apart — the
        // doorway between them is open, not crossed.
        assert!(left_moved.x < left_hinge.x, "left {left_moved:?}");
        assert!(right_moved.x > right_hinge.x, "right {right_moved:?}");
        assert!(right_moved.x - left_moved.x > 1.3);
    }

    #[test]
    fn swing_transform_keeps_hinge_fixed() {
        let closed = Transform::IDENTITY;
        let hinge = Vec3::new(0.686, 1.257, -0.057);
        for angle in [-0.314, -1.606, 0.5, -3.0] {
            let swung = swing_transform(&closed, hinge, angle);
            // The hinge point itself does not move.
            let moved = swung.transform_point(hinge);
            assert!((moved - hinge).length() < 1e-5, "angle {angle}: {moved:?}");
        }
    }

    #[test]
    fn swing_transform_moves_free_edge_like_the_nif() {
        // FarmhouseLDoor01 geometry: hinge at (48, 4, 88) Skyrim
        // units, panel free edge 96 units in -X from it. The NIF's
        // -18 deg rotation about Z moves the free edge toward
        // Skyrim +Y (computed from the rotation matrix by hand).
        // Bevy -Z IS Skyrim +Y, so the Bevy swing must move the
        // free edge toward -Z — the un-negated angle.
        let closed = Transform::IDENTITY;
        let hinge = Vec3::new(48.0 / 70.0, 88.0 / 70.0, -4.0 / 70.0);
        let free_edge = Vec3::new(-48.0 / 70.0, 88.0 / 70.0, -4.0 / 70.0);
        let swung = swing_transform(&closed, hinge, -0.31415927);
        let moved = swung.transform_point(free_edge);
        // Free edge moved toward -Z (Skyrim +Y), as the NIF does.
        assert!(moved.z < free_edge.z - 0.3, "moved {moved:?}");
        // And stayed at the same height and near the same radius.
        assert!((moved.y - free_edge.y).abs() < 1e-5);
        let r0 = (free_edge - hinge).length();
        let r1 = (moved - hinge).length();
        assert!((r0 - r1).abs() < 1e-5);
    }

    #[test]
    fn doorway_crossing_fires_on_walk_through() {
        let door = test_door();
        // Straight through the middle.
        assert!(doorway_crossing(
            &door,
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, 1.0)
        ));
        // Approaching but not crossing.
        assert!(!doorway_crossing(
            &door,
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, -0.5)
        ));
        // Crossing outside the doorway width (around the side).
        assert!(!doorway_crossing(
            &door,
            Vec3::new(2.0, 0.0, -1.0),
            Vec3::new(2.0, 0.0, 1.0)
        ));
        // Crossing above the doorway (flying over the door).
        assert!(!doorway_crossing(
            &door,
            Vec3::new(0.0, 3.0, -1.0),
            Vec3::new(0.0, 3.0, 1.0)
        ));
        // Walking back the other way also crosses.
        assert!(doorway_crossing(
            &door,
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, -1.0)
        ));
    }

    #[test]
    fn door_blocks_move_closed_vs_open() {
        let mut door = test_door();
        let from = Vec3::new(0.0, 0.0, -1.0);
        let to = Vec3::new(0.0, 0.0, 1.0);
        // Closed: blocked.
        assert!(door_blocks_move(&door, from, to));
        // A crack (less than half the 18 deg swing): still blocked.
        door.angle = -0.1;
        assert!(door_blocks_move(&door, from, to));
        // Past half swing: walk through.
        door.angle = -0.2;
        assert!(!door_blocks_move(&door, from, to));
        // Fully open: walk through.
        door.angle = -0.31415927;
        assert!(!door_blocks_move(&door, from, to));
    }

    #[test]
    fn swinging_door_fires_closed_never_fires() {
        let mut door = test_door();
        let from = Vec3::new(0.0, 0.0, -1.0);
        let to = Vec3::new(0.0, 0.0, 1.0);
        // Closed + walking through the plane: no teleport.
        assert!(!swinging_door_fires(&door, Some(from), to));
        // A crack (less than half the 18 deg swing): still no
        // teleport — the player is blocked (see
        // door_blocks_move_closed_vs_open) and the transition
        // must not fire under them either.
        door.angle = -0.1;
        assert!(!swinging_door_fires(&door, Some(from), to));
    }

    #[test]
    fn swinging_door_fires_open_crossing_fires() {
        let mut door = test_door();
        let from = Vec3::new(0.0, 0.0, -1.0);
        let to = Vec3::new(0.0, 0.0, 1.0);
        // Past half swing + crossing: the teleport fires.
        door.angle = -0.2;
        assert!(swinging_door_fires(&door, Some(from), to));
        // Fully open + crossing the other way: also fires
        // (both directions work through the same code path).
        door.angle = -0.31415927;
        assert!(swinging_door_fires(&door, Some(to), from));
    }

    #[test]
    fn swinging_door_fires_from_below_within_two_metres() {
        // The inn-return defense-in-depth: the porch is 1.35 m
        // above the street, so a player crossing the doorway
        // plane from street level (feet at base_y - 1.35) must
        // still fire. Beyond 2.0 m below, no fire.
        let mut door = test_door();
        door.angle = -0.31415927;
        // Street level: fires.
        assert!(swinging_door_fires(
            &door,
            Some(Vec3::new(0.0, -1.35, -1.0)),
            Vec3::new(0.0, -1.35, 1.0)
        ));
        // Just inside the window: fires.
        assert!(swinging_door_fires(
            &door,
            Some(Vec3::new(0.0, -1.99, -1.0)),
            Vec3::new(0.0, -1.99, 1.0)
        ));
        // Beyond the window: no fire.
        assert!(!swinging_door_fires(
            &door,
            Some(Vec3::new(0.0, -2.01, -1.0)),
            Vec3::new(0.0, -2.01, 1.0)
        ));
    }

    #[test]
    fn swinging_door_fires_needs_crossing_and_xtel() {
        let mut door = test_door();
        door.angle = -0.31415927;
        // Open but standing still inside the trigger: no
        // crossing, no teleport — USE never teleports the player
        // on the spot.
        assert!(!swinging_door_fires(
            &door,
            Some(Vec3::new(0.0, 0.0, 0.5)),
            Vec3::new(0.0, 0.0, 0.5)
        ));
        // No previous frame (just arrived): no teleport.
        assert!(!swinging_door_fires(&door, None, Vec3::new(0.0, 0.0, 1.0)));
        // A swinging room door (no XTEL) never teleports, however
        // open: the player just walks through it.
        door.has_xtel = false;
        assert!(!swinging_door_fires(
            &door,
            Some(Vec3::new(0.0, 0.0, -1.0)),
            Vec3::new(0.0, 0.0, 1.0)
        ));
    }

    #[test]
    fn nearest_door_picks_the_closest_in_range() {
        let mut doors = Doors::default();
        let mut near = test_door();
        near.ref_form_id = 0x1;
        near.plane_point = Vec3::new(1.0, 0.0, 0.0);
        let mut far = test_door();
        far.ref_form_id = 0x2;
        far.plane_point = Vec3::new(2.0, 0.0, 0.0);
        let mut out = test_door();
        out.ref_form_id = 0x3;
        out.plane_point = Vec3::new(10.0, 0.0, 0.0);
        doors.doors.insert(0x1, near);
        doors.doors.insert(0x2, far);
        doors.doors.insert(0x3, out);
        assert_eq!(nearest_door_in_range(&doors, Vec3::ZERO), Some(0x1));
        assert_eq!(
            nearest_door_in_range(&doors, Vec3::new(20.0, 0.0, 0.0)),
            None
        );
    }

    #[test]
    fn open_angle_from_nif_picks_open_sequence() {
        // A synthetic door NIF: Open (Z 0 -> -0.3142 over 0.6 s)
        // then Close (Z -0.3142 -> 0 over 0.6 s), as in
        // FarmhouseLDoor01.nif.
        let key = |time: f32, value: f32| dovah_nif::FloatKey {
            time,
            value,
            fwd_tangent: 0.0,
            bwd_tangent: 0.0,
        };
        let flat = vec![key(0.0, 0.0), key(0.6, 0.0)];
        let open = dovah_nif::TransformDataBlock {
            x_keys: flat.clone(),
            y_keys: flat.clone(),
            z_keys: vec![key(0.0, 0.0), key(0.6, -0.31415927)],
            x_key_type: dovah_nif::KeyInterpolation::Linear,
            y_key_type: dovah_nif::KeyInterpolation::Linear,
            z_key_type: dovah_nif::KeyInterpolation::Linear,
            translation_keys: Vec::new(),
            translation_key_type: dovah_nif::KeyInterpolation::Constant,
        };
        let close = dovah_nif::TransformDataBlock {
            x_keys: flat.clone(),
            y_keys: flat.clone(),
            z_keys: vec![key(0.0, -0.31415927), key(0.6, 0.0)],
            x_key_type: dovah_nif::KeyInterpolation::Linear,
            y_key_type: dovah_nif::KeyInterpolation::Linear,
            z_key_type: dovah_nif::KeyInterpolation::Linear,
            translation_keys: Vec::new(),
            translation_key_type: dovah_nif::KeyInterpolation::Constant,
        };
        let nif = dovah_nif::NifFile {
            version: 0x1402_0007,
            user_version: 12,
            bs_version: 100,
            header_string: String::new(),
            author: String::new(),
            process_script: String::new(),
            export_script: String::new(),
            strings: Vec::new(),
            blocks: vec![
                dovah_nif::Block::TransformData(open),
                dovah_nif::Block::TransformData(close),
            ],
            root_indices: vec![0],
        };
        let (angle, duration) = open_angle_from_nif(&nif).unwrap();
        assert!((angle - (-0.31415927)).abs() < 1e-6);
        assert_eq!(duration, 0.6);
        // No TransformData at all: None (the caller falls back).
        let empty = dovah_nif::NifFile {
            blocks: Vec::new(),
            ..nif
        };
        assert!(open_angle_from_nif(&empty).is_none());
    }

    #[test]
    // -6.2832 is the REAL keyframe value from FarmhouseAnimDoor02's
    // NIF, not a hand-typed approximation of TAU — replacing it
    // with the constant would falsify the test data.
    #[allow(clippy::approx_constant)]
    fn open_angle_from_nif_handles_minus_two_pi_rest() {
        // FarmhouseAnimDoor02 authors its rest state as -2π: Open
        // runs -6.2832 -> -4.6775 (a +92 deg swing from the
        // opposite hinge), Close runs back. The open angle is the
        // DELTA, and rest is tested modulo 2π.
        let key = |time: f32, value: f32| dovah_nif::FloatKey {
            time,
            value,
            fwd_tangent: 0.0,
            bwd_tangent: 0.0,
        };
        let flat = vec![key(0.0, 0.0), key(1.0, 0.0)];
        let open = dovah_nif::TransformDataBlock {
            x_keys: flat.clone(),
            y_keys: flat.clone(),
            z_keys: vec![key(0.0, -6.2832), key(0.83, -4.6251), key(1.0, -4.6775)],
            x_key_type: dovah_nif::KeyInterpolation::Linear,
            y_key_type: dovah_nif::KeyInterpolation::Linear,
            z_key_type: dovah_nif::KeyInterpolation::Linear,
            translation_keys: Vec::new(),
            translation_key_type: dovah_nif::KeyInterpolation::Constant,
        };
        let close = dovah_nif::TransformDataBlock {
            x_keys: flat.clone(),
            y_keys: flat.clone(),
            z_keys: vec![key(0.0, -4.6775), key(1.0, -6.2832)],
            x_key_type: dovah_nif::KeyInterpolation::Linear,
            y_key_type: dovah_nif::KeyInterpolation::Linear,
            z_key_type: dovah_nif::KeyInterpolation::Linear,
            translation_keys: Vec::new(),
            translation_key_type: dovah_nif::KeyInterpolation::Constant,
        };
        let nif = dovah_nif::NifFile {
            version: 0x1402_0007,
            user_version: 12,
            bs_version: 100,
            header_string: String::new(),
            author: String::new(),
            process_script: String::new(),
            export_script: String::new(),
            strings: Vec::new(),
            blocks: vec![
                dovah_nif::Block::TransformData(open),
                dovah_nif::Block::TransformData(close),
            ],
            root_indices: vec![0],
        };
        let (angle, duration) = open_angle_from_nif(&nif).unwrap();
        assert!((angle - 1.6057).abs() < 1e-3, "angle {angle}");
        assert_eq!(duration, 1.0);
    }
}
