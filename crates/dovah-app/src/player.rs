//! First-person controller (windowed mode only).
//!
//! Mouse look + WASD movement with gravity, clamped to the room
//! bounds. The player walks on the floor ([`FloorGrid` heights
//! from the real meshes), jumps with Space / the jump button, and
//! sprints with Shift / the sprint button while stamina lasts.
//! Walls and furniture block horizontally ([`ObstacleGrid`]:
//! steep collision triangles + primitive footprints from the
//! game's own Havok data); the room-bounds clamp is the outer
//! wall. Controls: WASD move, Space jump, Shift sprint, Esc
//! release the mouse, click to grab it again.
//!
//! M8: the same systems also read [`TouchInputState`] when the
//! touch plugin provides it (Android only) — joystick move,
//! drag-to-look, jump/sprint buttons. On desktop the resource is
//! absent and the `Option<Res<..>>` reads are `None`.
//!
//! M8.1: gravity + grounded walking replaced the M6/M8 fly mode
//! (Dylan: movement felt floaty, no-clip-like). Jump is a real
//! jump now (upward velocity, gravity brings you back down); the
//! old ascend/descend keys are gone.
//!
//! M15: a closed door blocks passage — movement that would carry
//! the player through a closed door's plane is cancelled (see
//! [`doors::door_blocks_move`]). USE (E / the USE button) toggles
//! the nearest door; that lives in [`doors::door_use`].

use crate::collision::ObstacleGrid;
use crate::scene::{FloorGrid, Player, RoomBounds, TerrainHeights};
use crate::touch::TouchInputState;
use bevy::app::{App, Plugin, Startup, Update};
use bevy::ecs::prelude::*;
use bevy::input::keyboard::KeyCode;
use bevy::input::mouse::MouseMotion;
use bevy::input::ButtonInput;
use bevy::math::Vec3;
use bevy::prelude::in_state;
use bevy::time::Time;
use bevy::transform::components::Transform;
use bevy::window::{CursorGrabMode, CursorOptions, Window};

/// Gravity, m/s^2. Stronger than Earth's 9.8 for a snappy game
/// feel (Skyrim's own gravity is similarly game-tuned).
pub const GRAVITY: f32 = 22.0;
/// Jump impulse, m/s. Reaches ~1.1 m (`v^2 / 2g`), about a
/// Skyrim jump.
pub const JUMP_VELOCITY: f32 = 7.0;
/// Camera height above the feet, in metres. The player entity's
/// translation is the eye; the feet are this far below it.
pub const EYE_HEIGHT: f32 = 1.7;
/// Largest rise the player steps up without jumping — stair
/// treads are ~0.26 m, so this climbs stairs and door sills while
/// keeping tables and counters unclimbable. It is also the bottom
/// of the obstacle band ([`ObstacleGrid::blocked`]): an obstacle
/// shorter than this never blocks, so a surface you can step
/// onto never blocks you and a surface that blocks you is never
/// steppable.
///
/// Measured, not guessed (collision_design probe, 2026-11-16):
/// the inn porch's collision steps rise 0.26 m each (base plate
/// to top landing, 7 treads), while the lowest furniture seat is
/// 0.42 m (bench) / 0.43 m (chair) — 0.3 sits between them with
/// room on both sides. (The earlier 0.52 m figure was a
/// VISUAL-mesh rasterization artifact: the 1 m cells merged
/// pairs of treads. The collision mesh, which the FloorGrid now
/// uses when present, has the true 0.26 m treads.)
pub const STEP_HEIGHT: f32 = 0.3;
/// The player's height, in metres — the top of the obstacle band
/// ([`ObstacleGrid::blocked`]): a beam above this passes over
/// the head, a beam below it blocks. Same as the eye height:
/// the head is at the eye for this purpose.
pub const PLAYER_HEIGHT: f32 = 1.7;
/// Stamina scale (0–100, like the bar).
pub const STAMINA_MAX: f32 = 100.0;
/// Stamina drained per second of sprinting (10 s of sprint).
pub const STAMINA_DRAIN_PER_SEC: f32 = 10.0;
/// Stamina regained per second while not sprinting.
pub const STAMINA_REGEN_PER_SEC: f32 = 12.0;

/// Vertical physics state, kept on the player entity.
#[derive(Component, Default)]
pub struct PlayerPhysics {
    /// Vertical velocity, m/s (positive up).
    pub vy: f32,
    /// On the ground (feet at the floor height).
    pub grounded: bool,
}

/// Stamina (0–[`STAMINA_MAX`]): drains while sprinting, regens
/// otherwise. The touch UI reads it for the stamina bar (M8.1).
#[derive(Resource)]
pub struct Stamina {
    pub value: f32,
}

impl Default for Stamina {
    fn default() -> Self {
        Stamina { value: STAMINA_MAX }
    }
}

/// One stamina step: drain while sprinting, regen otherwise.
/// Pure — unit-tested without a Bevy app.
pub fn step_stamina(value: f32, sprinting: bool, dt: f32) -> f32 {
    if sprinting {
        (value - STAMINA_DRAIN_PER_SEC * dt).max(0.0)
    } else {
        (value + STAMINA_REGEN_PER_SEC * dt).min(STAMINA_MAX)
    }
}

/// One vertical-physics step for the *feet* position: apply a jump
/// impulse (grounded only), integrate gravity, land on the ground.
/// Returns `(new_feet_y, new_vy, grounded)`. Pure — unit-tested
/// without a Bevy app. The caller supplies `ground`, the floor
/// height under the player (already step-adjusted for walking).
pub fn step_vertical(
    feet_y: f32,
    vy: f32,
    grounded: bool,
    jump_pressed: bool,
    dt: f32,
    ground: f32,
) -> (f32, f32, bool) {
    let mut vy = vy;
    let mut grounded = grounded;
    if grounded && jump_pressed {
        vy = JUMP_VELOCITY;
        grounded = false;
    }
    vy -= GRAVITY * dt;
    let mut y = feet_y + vy * dt;
    if y <= ground {
        y = ground;
        vy = 0.0;
        grounded = true;
    }
    (y, vy, grounded)
}

/// Mouse-look state, kept on the player entity.
#[derive(Component, Default)]
pub struct LookAngles {
    pub yaw: f32,
    pub pitch: f32,
}

impl LookAngles {
    /// Build yaw/pitch from a look direction, matching the YXZ
    /// Euler order used by `mouse_look`.
    pub fn from_forward(forward: Vec3) -> Self {
        let f = forward.normalize();
        LookAngles {
            yaw: (-f.x).atan2(-f.z),
            pitch: f.y.asin(),
        }
    }
}

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Stamina>()
            .add_systems(Startup, grab_mouse)
            .add_systems(Update, (mouse_look, keyboard_move, cursor_toggle).chain())
            // Water flow: the river's UV scroll (water.rs).
            // Registered here — not in lib.rs — because the
            // windowed path adds PlayerPlugin and the
            // screenshot path adds ScreenshotPlugin, which
            // carries the same line: the two registrations
            // together cover both paths.
            .add_systems(
                Update,
                crate::water::animate_water_flow.run_if(in_state(crate::AppState::InGame)),
            )
            .add_systems(
                Update,
                crate::mill::animate_mill_wheel.run_if(in_state(crate::AppState::InGame)),
            )
            .add_systems(
                Update,
                crate::sawmill::animate_sawmill_blade.run_if(in_state(crate::AppState::InGame)),
            );
    }
}

fn grab_mouse(mut cursors: Query<&mut CursorOptions, With<Window>>) {
    if let Ok(mut cursor) = cursors.single_mut() {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
}

fn cursor_toggle(
    input: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<bevy::input::mouse::MouseButton>>,
    mut cursors: Query<&mut CursorOptions, With<Window>>,
) {
    let Ok(mut cursor) = cursors.single_mut() else {
        return;
    };
    if input.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    } else if mouse.just_pressed(bevy::input::mouse::MouseButton::Left)
        && cursor.grab_mode == CursorGrabMode::None
    {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
}

/// Mouse look + touch drag-to-look. Public so the touch plugin
/// can order its tracking system before it (M8).
#[allow(clippy::too_many_arguments)]
pub fn mouse_look(
    mut motion: MessageReader<MouseMotion>,
    touch: Option<Res<TouchInputState>>,
    // M17: the inventory open means the mouse is for the UI, not
    // the camera. Optional — the plugin may not be registered
    // (the unit tests' minimal app has no inventory).
    inventory_open: Option<Res<crate::inventory::InventoryOpen>>,
    // M18: same for dialogue — the mouse picks topics, not the
    // camera.
    dialogue_open: Option<Res<crate::dialogue::DialogueOpen>>,
    // UI milestone 1: same for the pause screen — and the
    // settings (sensitivity + invert Y) that mouse look reads.
    pause_open: Option<Res<crate::pause::PauseOpen>>,
    // UI milestone 2: same for the character menu.
    character_open: Option<Res<crate::character::CharacterMenuOpen>>,
    settings: Option<Res<crate::pause::GameSettings>>,
    mut query: Query<(&mut Transform, &mut LookAngles), With<Player>>,
) {
    if inventory_open.map(|o| o.0).unwrap_or(false)
        || dialogue_open.map(|o| o.0).unwrap_or(false)
        || pause_open.map(|o| o.0).unwrap_or(false)
        || character_open.map(|o| o.0).unwrap_or(false)
    {
        // Drain the motion queue so a backlog does not spin the
        // camera when the menu closes.
        for _ in motion.read() {}
        return;
    }
    let Ok((mut transform, mut angles)) = query.single_mut() else {
        return;
    };
    // Base sensitivity 0.0025 rad/px, scaled by the pause
    // screen's Look Sensitivity stepper (default 100%) and
    // flipped by Invert Y.
    let (sensitivity, invert_y) = match &settings {
        Some(s) => (0.0025 * s.sensitivity.multiplier(), s.invert_y),
        None => (0.0025, false),
    };
    let pitch_sign = if invert_y { 1.0 } else { -1.0 };
    for event in motion.read() {
        angles.yaw -= event.delta.x * sensitivity;
        angles.pitch = (angles.pitch + pitch_sign * event.delta.y * sensitivity).clamp(-1.5, 1.5);
    }
    // M8: touch drag deltas are the same units and sign as mouse
    // motion, so they feed the same angles.
    if let Some(touch) = touch {
        angles.yaw -= touch.look_delta.x * sensitivity;
        angles.pitch =
            (angles.pitch + pitch_sign * touch.look_delta.y * sensitivity).clamp(-1.5, 1.5);
    }
    transform.rotation =
        bevy::math::Quat::from_euler(bevy::math::EulerRot::YXZ, angles.yaw, angles.pitch, 0.0);
}

/// Keyboard + touch movement, gravity, jumping, and stamina.
/// Public so the touch plugin can order its tracking system
/// before it (M8).
///
/// Horizontal movement is the M6/M8 scheme (WASD + joystick,
/// sprint = 3x); vertical movement is M8.1 gravity: the feet fall
/// to the [`FloorGrid`] height under the player, Space / the jump
/// button is a real jump (edge-triggered, grounded only), and
/// there is no ascend or descend any more. Stamina drains while
/// sprinting and regens otherwise; an empty bar means no sprint.
// Bevy systems take their inputs as parameters by design; the
// count is the dependency list, not a smell to refactor away.
#[allow(clippy::too_many_arguments)]
pub fn keyboard_move(
    time: Res<Time>,
    input: Res<ButtonInput<KeyCode>>,
    touch: Option<Res<TouchInputState>>,
    // v0.2.2: `RoomBounds` is inserted only when the worker thread
    // delivers the cell data — it does NOT exist during the
    // Loading state. As a required `Res` this was the v0.2.0/v0.2.1
    // instant crash on Dylan's phone: Bevy treats a missing
    // required resource as a validation failure and its default
    // error handler panics, which android-activity turns into
    // SIGABRT before the first frame. Optional, like the
    // `FloorGrid` next to it: no bounds, no movement yet.
    bounds: Option<Res<RoomBounds>>,
    grid: Option<Res<FloorGrid>>,
    // Per-object collision: steep collision triangles +
    // footprints. Absent before the first scene spawns (the same
    // pattern as the FloorGrid next to it): no grid, no blocking.
    obstacles: Option<Res<ObstacleGrid>>,
    // M9: terrain heights exist only outdoors. The ground under
    // the player is the higher of the mesh floor (bridge, roof,
    // porch) and the terrain — the same rule the grid already
    // applies between mesh tops.
    terrain: Option<Res<TerrainHeights>>,
    // M15: swinging doors, for the closed-door block. Absent
    // before the first scene spawns.
    doors: Option<Res<crate::doors::Doors>>,
    // M17: the inventory open freezes the player (Skyrim pauses
    // the world on the menu). Optional — the plugin may not be
    // registered.
    inventory_open: Option<Res<crate::inventory::InventoryOpen>>,
    // M18: same for dialogue — the player stands still while
    // talking.
    dialogue_open: Option<Res<crate::dialogue::DialogueOpen>>,
    // UI milestone 1: same for the pause screen — Skyrim pauses
    // the world on the menu.
    pause_open: Option<Res<crate::pause::PauseOpen>>,
    // UI milestone 2: same for the character menu.
    character_open: Option<Res<crate::character::CharacterMenuOpen>>,
    mut stamina: ResMut<Stamina>,
    mut query: Query<(&mut Transform, &mut PlayerPhysics), With<Player>>,
) {
    if inventory_open.map(|o| o.0).unwrap_or(false)
        || dialogue_open.map(|o| o.0).unwrap_or(false)
        || pause_open.map(|o| o.0).unwrap_or(false)
        || character_open.map(|o| o.0).unwrap_or(false)
    {
        return;
    }
    let Ok((mut transform, mut physics)) = query.single_mut() else {
        return;
    };
    let Some(bounds) = bounds else {
        return;
    };
    let dt = time.delta_secs();

    // --- horizontal wish (camera-relative, ground plane) ---
    // The forward/right vectors are flattened to the ground plane
    // first: looking up or down must not send the player flying
    // (the old fly mode used the full forward vector).
    let forward = transform.forward();
    let right = transform.right();
    let fwd = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let rgt = Vec3::new(right.x, 0.0, right.z).normalize_or_zero();
    let mut wish = Vec3::ZERO;
    let mut keyboard_moving = false;
    if input.pressed(KeyCode::KeyW) {
        wish += fwd;
        keyboard_moving = true;
    }
    if input.pressed(KeyCode::KeyS) {
        wish -= fwd;
        keyboard_moving = true;
    }
    if input.pressed(KeyCode::KeyD) {
        wish += rgt;
        keyboard_moving = true;
    }
    if input.pressed(KeyCode::KeyA) {
        wish -= rgt;
        keyboard_moving = true;
    }
    let mut touch_speed = 1.0f32;
    let mut touch_sprint = false;
    if let Some(ref touch) = touch {
        if touch.move_vec != bevy::math::Vec2::ZERO {
            // Joystick: analog — a half-deflected stick moves at
            // half speed.
            wish += fwd * touch.move_vec.y + rgt * touch.move_vec.x;
            touch_speed = touch.move_vec.length().clamp(0.0, 1.0);
        }
        touch_sprint = touch.sprint;
    }
    let moving = wish != Vec3::ZERO;

    // --- sprint + stamina ---
    let want_sprint =
        input.pressed(KeyCode::ShiftLeft) || input.pressed(KeyCode::ShiftRight) || touch_sprint;
    let sprinting = want_sprint && moving && stamina.value > 0.0;
    stamina.value = step_stamina(stamina.value, sprinting, dt);
    let sprint = if sprinting { 3.0 } else { 1.0 };
    // Keyboard input is digital (full speed); a lone joystick is
    // analog, so its deflection sets the speed.
    let analog = if keyboard_moving { 1.0 } else { touch_speed };
    let speed = 4.0 * sprint * analog; // m/s

    let mut next = transform.translation;
    if moving {
        next += wish.normalize_or_zero() * speed * dt;
    }

    // The feet height for the horizontal door block: the CURRENT
    // feet — next.y is still the old eye height here (the
    // vertical step below has not run yet).
    let feet_y = transform.translation.y - EYE_HEIGHT;

    // M15: a closed door blocks passage — a move that would carry
    // the player through a closed door's plane is cancelled
    // (horizontal only; the vertical step below is untouched).
    if let Some(doors) = &doors {
        let current_feet = Vec3::new(transform.translation.x, feet_y, transform.translation.z);
        let next_feet = Vec3::new(next.x, current_feet.y, next.z);
        for door in doors.doors.values() {
            if crate::doors::door_blocks_move(door, current_feet, next_feet) {
                next.x = transform.translation.x;
                next.z = transform.translation.z;
                break;
            }
        }
    }

    // --- vertical: jump + gravity + ground ---
    let jump_pressed = input.just_pressed(KeyCode::Space)
        || touch.as_deref().map(|t| t.jump_pressed).unwrap_or(false);
    // Step-up applies while walking (grounded): the probe height
    // is the feet plus STEP_HEIGHT, so stairs and sills lift the
    // player. While airborne the probe is the feet exactly, so a
    // fall lands on the surface rather than 0.3 m above it.
    let probe = if physics.grounded {
        feet_y + STEP_HEIGHT
    } else {
        feet_y
    };
    let mut ground = match &grid {
        Some(grid) => grid.ground_height(next.x, next.z, probe),
        None => bounds.floor_y,
    };
    // M9: outdoors the terrain is the floor the meshes sit on.
    // (Inside, `terrain` is absent and this is a no-op.)
    if let Some(terrain) = &terrain {
        if let Some(th) = terrain.height_at(next.x, next.z) {
            ground = ground.max(th);
        }
    }
    let (new_feet_y, new_vy, grounded) = step_vertical(
        feet_y,
        physics.vy,
        physics.grounded,
        jump_pressed,
        dt,
        ground,
    );
    let vy_before = physics.vy;
    let grounded_before = physics.grounded;
    physics.vy = new_vy;
    physics.grounded = grounded;
    next.y = new_feet_y + EYE_HEIGHT;

    // Per-object collision: AFTER the vertical step, so the band
    // uses the feet height the player is moving TO. A stair riser
    // tops out below the band from the tread above it, but pokes
    // into the band from the tread below — checking from the old
    // feet blocks every stair (the first walk sim failed on the
    // porch's first riser for exactly this reason). The band
    // bottom follows the step-up probe: grounded, feet +
    // STEP_HEIGHT (stairs pass under it); airborne, the feet
    // themselves (a jumping player hits the table side). On a
    // block the whole move is cancelled — position AND vertical
    // state — the player never left the old spot.
    if let Some(obstacles) = &obstacles {
        let band_lo = if grounded {
            new_feet_y + STEP_HEIGHT
        } else {
            new_feet_y
        };
        if obstacles.blocked(next.x, next.z, band_lo, new_feet_y + PLAYER_HEIGHT) {
            next = transform.translation;
            physics.vy = vy_before;
            physics.grounded = grounded_before;
        }
    }

    // Stay inside the room (a small margin keeps the camera out of
    // the walls). The floor is the ground grid now, not a clamp.
    let margin = 0.3;
    next.x = next.x.clamp(bounds.min.x + margin, bounds.max.x - margin);
    next.z = next.z.clamp(bounds.min.z + margin, bounds.max.z - margin);
    next.y = next.y.min(bounds.max.y - margin);
    transform.translation = next;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fall_lands_on_the_ground() {
        // Feet 2 m up, no jump: falls and lands exactly on the
        // ground, grounded, no residual velocity.
        let (mut y, mut vy, mut grounded) = (2.0, 0.0, false);
        for _ in 0..120 {
            let step = step_vertical(y, vy, grounded, false, 1.0 / 60.0, 0.0);
            (y, vy, grounded) = step;
            if grounded {
                break;
            }
        }
        assert!(grounded);
        assert_eq!(y, 0.0);
        assert_eq!(vy, 0.0);
    }

    #[test]
    fn jump_rises_then_lands_back() {
        // Grounded jump: rises ~1.1 m, comes back down, lands.
        let (mut y, mut vy, mut grounded) = (0.0, 0.0, true);
        let mut peak = 0.0f32;
        let mut jump_used = false;
        for _ in 0..240 {
            let press = !jump_used;
            jump_used = true;
            let step = step_vertical(y, vy, grounded, press, 1.0 / 60.0, 0.0);
            (y, vy, grounded) = step;
            peak = peak.max(y);
            if grounded && jump_used && y == 0.0 && vy == 0.0 {
                break;
            }
        }
        // v^2/2g = 49/44 = 1.11 m, minus one frame of gravity.
        assert!(peak > 1.0 && peak < 1.15, "peak {peak}");
        assert!(grounded);
        assert_eq!(y, 0.0);
    }

    #[test]
    fn jump_does_nothing_mid_air() {
        // Pressing jump while airborne changes nothing (no
        // double-jump): same fall as without the press.
        let (y1, vy1, g1) = step_vertical(2.0, 0.0, false, true, 1.0 / 60.0, 0.0);
        let (y2, vy2, g2) = step_vertical(2.0, 0.0, false, false, 1.0 / 60.0, 0.0);
        assert_eq!((y1, vy1, g1), (y2, vy2, g2));
    }

    #[test]
    fn grounded_without_jump_stays_put() {
        // Standing on the ground, no input: no sinking, no bounce.
        let (y, vy, grounded) = step_vertical(0.0, 0.0, true, false, 1.0 / 60.0, 0.0);
        assert_eq!((y, vy, grounded), (0.0, 0.0, true));
    }

    /// v0.2.2 regression test: the player plugin must run while
    /// `RoomBounds` does not exist yet. The M8.1 loading screen
    /// inserts `RoomBounds` only when the worker thread delivers
    /// the cell data, but `keyboard_move` required `Res<RoomBounds>`
    /// from the first Update tick — Bevy treats a missing required
    /// resource as a validation failure and (default error handler)
    /// panics, which android-activity turns into SIGABRT before
    /// the first frame. That was the v0.2.0/v0.2.1 instant crash on
    /// Dylan's phone: the message screen (no PlayerPlugin) ran
    /// fine, the main app died immediately. Two updates with no
    /// `RoomBounds` and no player entity must not panic.
    #[test]
    fn player_plugin_runs_without_room_bounds() {
        use bevy::app::App;
        use bevy::input::mouse::MouseMotion;
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .add_plugins(bevy::input::InputPlugin)
            .add_message::<MouseMotion>()
            .add_plugins(PlayerPlugin);
        // No RoomBounds resource, no Player entity: the loading
        // state. Must not panic.
        app.update();
        app.update();
    }

    #[test]
    fn stamina_drains_then_regens() {
        let mut value = STAMINA_MAX;
        // 5 s of sprinting: 100 -> 50.
        for _ in 0..300 {
            value = step_stamina(value, true, 1.0 / 60.0);
        }
        assert!((value - 50.0).abs() < 0.01, "value {value}");
        // 5 s of rest: 50 -> 100 (capped).
        for _ in 0..300 {
            value = step_stamina(value, false, 1.0 / 60.0);
        }
        assert_eq!(value, STAMINA_MAX);
        // Drain never goes below zero.
        for _ in 0..1200 {
            value = step_stamina(value, true, 1.0 / 60.0);
        }
        assert_eq!(value, 0.0);
    }
}
