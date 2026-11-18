//! The real Skyrim HUD (Ruffle M4): `hudmenu.gfx`, unmodified,
//! rendered inside the engine on Bevy's wgpu device and composited
//! over the 3D scene, driven by live game state.
//!
//! Architecture (forced by Bevy 0.20 internals — see
//! `docs/flash-interface-feasibility.md` §5/M4):
//!
//! * The Ruffle player lives in the RENDER world. Bevy creates its
//!   wgpu device asynchronously (`RenderCreation::Automatic` +
//!   `FutureRenderResources`, which is `pub(crate)`), so the main
//!   world cannot grab it at plugin time. A render system builds
//!   the player lazily from the extracted SWF/fontlib bytes, on
//!   Bevy's device — shared, no second device, no second shader
//!   compile (wgpu 30 unifies across Bevy and Ruffle).
//! * The main world drives it. `drive_ruffle_hud_state` snapshots
//!   live game state into [`HudState`] every InGame frame; an
//!   extract system copies it to the render world. The render
//!   system calls the GameDelegate handlers (meters on change,
//!   compass every frame, location/crosshair/hint on change) and
//!   steps the movie at its own 30 fps via a time accumulator —
//!   `run_frame` sets `needs_render` unconditionally, so the step
//!   itself must be gated, not just the render.
//! * Compositing is CPU readback + a Bevy UI Image: the render
//!   system captures the frame into a mailbox; the main world
//!   writes the pixels into an `Image` asset shown by a fullscreen
//!   `ImageNode` (transparent window mode, so the 3D scene shows
//!   through). One frame of latency. The GPU-composite pass
//!   (custom render node + shader quad) is M6 work.
//! * The Bevy HUD stays. While the Ruffle HUD is active, its
//!   display elements (bars, compass, crosshair —
//!   [`crate::touch::BevyHudDisplay`]) hide; if the Ruffle player
//!   never builds (missing SWF, no device), the Bevy HUD stays
//!   visible — the fallback. The Ruffle HUD is gated by the
//!   `ruffle_hud` setting (windowed path) or the `--ruffle-hud`
//!   CLI flag (screenshot path, via [`RuffleHudCliOverride`]).
//! * The fontlib is synthesized at runtime from the game's
//!   `fonts_en.swf` (Patch.bsa's copy, via
//!   `AssetProvider::read_file_latest`) — no hardcoded dev path.
//!
//! M4 limitations (documented, not hidden): the HUD renders at the
//! screenshot target's size (screenshot path) or the window size
//! (windowed path), capped at 1280×720 per dimension (the SWF's
//! design size — a larger stage just upscales the same layout and
//! wastes readback bandwidth); a window resize after InGame entry
//! stretches the existing texture instead of rebuilding the
//! player; tutorial hints have no triggers yet (the channel is
//! wired, nothing drives it); the exterior location is hardcoded
//! to "Riverwood" (the only exterior block is Riverwood 5,-11 — a
//! proper LCTN lookup is future work).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bevy::app::{App, AppExit, Plugin, Update};
use bevy::asset::{Assets, Handle, RenderAssetUsages};
use bevy::camera::visibility::Visibility;
use bevy::ecs::prelude::*;
use bevy::ecs::system::SystemParam;
use bevy::image::Image;
use bevy::math::Vec3;
use bevy::pbr::ViewClusterBindings;
use bevy::picking::Pickable;
use bevy::prelude::{in_state, OnEnter};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::renderer::{RenderAdapter, RenderDevice, RenderInstance, RenderQueue};
use bevy::render::view::window::ExtractedWindow;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};
use bevy::time::Time;
use bevy::transform::components::Transform;
use bevy::ui::widget::ImageNode;
use bevy::ui::{Node, Val};
use bevy::window::{PrimaryWindow, Window};
use image::RgbaImage;
use ruffle_harness::{synthesize_fontlib, FrameTiming, Harness, SharedWgpu, Value, SETTLE_FRAMES};

use crate::assets::AssetProvider;
use crate::cell::{CellData, DoorTrigger, PlacedObject, SceneTarget};
use crate::dialogue::{
    is_facing_target, nearest_npc_in_range_facing, DialogueIndex, GameStrings, TalkableNpcs,
};
use crate::doors::{Doors, USE_RANGE_M};
use crate::interact::{in_use_range, Containers, Pickupables};
use crate::inventory::{prettify_edid, ItemCatalog, ItemDef};
use crate::pause::GameSettings;
use crate::player::{LookAngles, Stamina, EYE_HEIGHT};
use crate::scene::Player;
use crate::screenshot::ScreenshotTarget;
use crate::touch::{heading_degrees, BevyHudDisplay};
use crate::{AppState, SharedPlugin};

/// The HUD's design size: the SWF stage is 1280×720, and the
/// meters/compass layout is tuned to it. The render target is
/// capped at this size per dimension (see the module docs).
pub const HUD_MAX_WIDTH: u32 = 1280;
pub const HUD_MAX_HEIGHT: u32 = 720;

/// The movie's frame rate (the SWF is 30 fps). The render system
/// steps the movie on this clock, not Bevy's frame clock, so the
/// meter fade-in anims play at vanilla speed on a 60 fps display.
const MOVIE_FRAME_SECS: f32 = 1.0 / 30.0;

/// The game paths of the HUD SWF and the fonts file, read from the
/// BSA at runtime (Patch.bsa's copies — see
/// `AssetProvider::read_file_latest`).
const HUD_SWF_PATH: &str = "interface/exported/hudmenu.gfx";
const FONTS_SWF_PATH: &str = "interface/fonts_en.swf";

/// Screenshot-path override for the Ruffle HUD (M4): the screenshot
/// path inserts this from the `--ruffle-hud` CLI flag, and the
/// drive system prefers it over the setting — so a screenshot run
/// without the flag always shows the Bevy HUD, even when a saved
/// settings.json has `ruffle_hud: true`. The windowed path never
/// inserts it.
#[derive(Debug, Clone, Copy, Resource)]
pub struct RuffleHudCliOverride(pub bool);

/// The prepared Ruffle HUD inputs: the SWF + synthesized fontlib
/// bytes, the render size, the data dir (the movie's base URL for
/// relative imports), and the safe-area insets. Prepared once per
/// app run, on the first InGame entry. The byte vectors are `Arc`d
/// so the per-frame extract clone is just a refcount bump.
#[derive(Debug, Clone, Resource)]
pub struct RuffleHudAssets {
    pub width: u32,
    pub height: u32,
    pub hud_swf: Arc<Vec<u8>>,
    pub fontlib: Arc<Vec<u8>>,
    pub base_path: PathBuf,
    /// Safe-area insets in pixels: (left, top, right, bottom).
    /// Zero on desktop; the phone fills these from the Android
    /// safe-area API later (patch 0007).
    pub safe_insets: (u32, u32, u32, u32),
}

/// The current cell's display name for the HUD ("Sleeping Giant
/// Inn", "Riverwood"). Computed on every InGame entry (it changes
/// per scene) and read by the drive system.
#[derive(Debug, Clone, Resource)]
pub struct CurrentLocation(pub String);

/// What the crosshair points at: nothing, or a named target with
/// optional weight/value lines (items only — an NPC, door, or
/// container has no weight/value to show).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum CrosshairTarget {
    #[default]
    None,
    Target {
        name: String,
        weight: Option<f32>,
        cost: Option<i32>,
    },
}

/// The live game state the HUD renders, snapshotted every InGame
/// frame by the drive system and extracted to the render world.
/// Percents are 0–100 (the SWF's `Meter` expects 0–100, not 0–1 —
/// M2). `delta_secs` is the frame delta for the movie clock; it is
/// part of the snapshot so the render world needs no `Time`.
#[derive(Debug, Clone, PartialEq, Resource)]
pub struct HudState {
    pub health: f32,
    pub magicka: f32,
    pub stamina: f32,
    /// Compass heading, degrees clockwise from north (0 = N).
    pub heading_deg: f32,
    pub location: String,
    pub crosshair: CrosshairTarget,
    /// Tutorial hint text, or `None` for no hint. No hint triggers
    /// exist in the engine yet (M4) — the channel is wired and the
    /// drive passes `None`.
    pub hint: Option<String>,
    /// Whether the Ruffle HUD is enabled (setting, or CLI override
    /// on the screenshot path). The render system pauses stepping
    /// while this is false.
    pub enabled: bool,
    pub delta_secs: f32,
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            health: 100.0,
            magicka: 100.0,
            stamina: 100.0,
            heading_deg: 0.0,
            location: String::new(),
            crosshair: CrosshairTarget::None,
            hint: None,
            enabled: false,
            delta_secs: 0.0,
        }
    }
}

/// The render→main mailbox: the render system writes the captured
/// HUD frame here; the upload system takes it. One shared `Arc`,
/// inserted into both worlds at plugin build.
#[derive(Debug, Clone, Resource, Default)]
pub struct RuffleHudMailbox(pub Arc<Mutex<Option<RgbaImage>>>);

/// The Bevy `Image` asset the HUD pixels are written into, created
/// lazily on the first captured frame (the `RgbaImage` carries its
/// own dimensions — the main world needs no size config).
#[derive(Debug, Resource)]
struct RuffleHudTexture {
    handle: Handle<Image>,
}

/// Whether a Ruffle HUD frame has ever arrived. The Bevy display
/// elements hide only once this is true — if the player never
/// builds, the Bevy HUD stays (the fallback).
#[derive(Debug, Default, Resource)]
pub struct RuffleHudActive(pub bool);

/// Marker for the fullscreen UI node showing the Ruffle HUD texture.
#[derive(Component)]
struct RuffleHudRoot;

/// The Ruffle player, held in the render world (M4). `Harness`
/// contains a `Player`, which is `!Send` — hence a `NonSend`
/// resource. The player is built lazily on the first render frame
/// that has the extracted assets (building takes ~1–3 s: shader
/// compile + fontlib import), then settled for [`SETTLE_FRAMES`]
/// frames before `InitExtensions` runs and the drive starts.
#[derive(Default)]
struct RuffleHudPlayer {
    harness: Option<Harness>,
    /// The last state driven into the SWF (change detection).
    last_driven: Option<HudState>,
    settled_frames: u32,
    initialized: bool,
    /// Movie-clock accumulator (seconds since the last step).
    time_accum: f32,
    /// Whether the first captured frame was logged to the startup
    /// log (one-time diagnostic — see the capture block).
    first_frame_logged: bool,
    /// Whether a slow movie step was logged (one-time diagnostic —
    /// v0.4.9: the per-step cost on the phone is the open question
    /// behind the missing first frame).
    slow_step_logged: bool,
    /// Whether a `None` capture was logged (one-time diagnostic —
    /// v0.4.9: capture returning `None` was silent, so the log
    /// could not distinguish "never reached capture" from
    /// "capture failed").
    capture_none_logged: bool,
    /// Per-window Ruffle cost stats (v0.4.11 perf work): step +
    /// capture costs accumulated per 5 s window and logged to the
    /// startup log, so the phone's next run shows whether the HUD
    /// readback is the lag. Wall-clock window (`Instant`) — the
    /// render world has no `Time` resource.
    ruffle_stats: RuffleCostStats,
}

/// Accumulated Ruffle step/capture costs for one telemetry
/// window (v0.4.11 perf work). Plain counters — no allocation in
/// the hot path; the only allocation is the one formatted line
/// per window. `window_start` is `None` until the first step is
/// recorded (the settle frames are not counted — the first
/// settle step is already logged one-time, and settle is a
/// one-time load cost, not steady-state lag).
#[derive(Debug, Default)]
struct RuffleCostStats {
    window_start: Option<Instant>,
    window_index: u32,
    steps: u32,
    run_total_ms: f64,
    run_max_ms: f64,
    render_total_ms: f64,
    render_max_ms: f64,
    captures: u32,
    capture_total_ms: f64,
    capture_max_ms: f64,
}

impl RuffleCostStats {
    /// Record one movie step's cost split.
    fn record_step(&mut self, timing: &FrameTiming) {
        if self.window_start.is_none() {
            self.window_start = Some(Instant::now());
        }
        self.steps += 1;
        self.run_total_ms += timing.preload_run_ms;
        if timing.preload_run_ms > self.run_max_ms {
            self.run_max_ms = timing.preload_run_ms;
        }
        self.render_total_ms += timing.render_ms;
        if timing.render_ms > self.render_max_ms {
            self.render_max_ms = timing.render_ms;
        }
    }

    /// Record one capture's wall cost (the `map_async` + blocking
    /// `device.poll` + row copy + alpha unmultiply — see
    /// `WgpuRenderBackend::capture_frame`).
    fn record_capture(&mut self, capture_ms: f64) {
        self.captures += 1;
        self.capture_total_ms += capture_ms;
        if capture_ms > self.capture_max_ms {
            self.capture_max_ms = capture_ms;
        }
    }

    /// Format the window's summary line and reset, when the
    /// window is due (elapsed >= [`crate::perf::PERF_WINDOW_SECS`]).
    /// Returns `None` when the window is not due or no steps were
    /// recorded.
    fn flush(&mut self, now: Instant) -> Option<String> {
        let start = self.window_start?;
        if self.steps == 0
            || now.duration_since(start).as_secs_f64() < crate::perf::PERF_WINDOW_SECS
        {
            return None;
        }
        let steps = self.steps as f64;
        let captures = self.captures as f64;
        let capture_avg = if self.captures > 0 {
            self.capture_total_ms / captures
        } else {
            0.0
        };
        let line = format!(
            "perf ruffle: window {}: {} steps, run avg {:.1}ms max {:.1}ms, render avg {:.1}ms max {:.1}ms, {} captures, capture avg {:.1}ms max {:.1}ms",
            self.window_index,
            self.steps,
            self.run_total_ms / steps,
            self.run_max_ms,
            self.render_total_ms / steps,
            self.render_max_ms,
            self.captures,
            capture_avg,
            self.capture_max_ms,
        );
        self.window_index += 1;
        self.steps = 0;
        self.run_total_ms = 0.0;
        self.run_max_ms = 0.0;
        self.render_total_ms = 0.0;
        self.render_max_ms = 0.0;
        self.captures = 0;
        self.capture_total_ms = 0.0;
        self.capture_max_ms = 0.0;
        self.window_start = Some(now);
        Some(line)
    }
}

/// The Ruffle HUD plugin (M4). Main-world systems run InGame-only;
/// the render-world setup is skipped when there is no RenderApp
/// (the headless regression test in lib.rs uses MinimalPlugins).
///
/// The plugin also carries the v0.4.9 swapchain-teardown fix
/// (`extract_app_exit` + `drop_swapchain_texture_on_exit`): it is
/// NOT Ruffle-specific, but this is the only plugin with
/// render-app access, and the fix must run in the render world.
/// See the systems' docs for the full story.
pub struct RuffleHudPlugin;

impl Plugin for RuffleHudPlugin {
    fn build(&self, app: &mut App) {
        let mailbox = RuffleHudMailbox::default();
        app.init_resource::<HudState>()
            .init_resource::<RuffleHudActive>()
            .init_resource::<crate::perf::FrameStats>()
            .insert_resource(mailbox.clone())
            .add_systems(OnEnter(AppState::InGame), prepare_ruffle_hud_assets)
            .add_systems(OnEnter(AppState::InGame), update_current_location)
            .add_systems(
                Update,
                drive_ruffle_hud_state.run_if(in_state(AppState::InGame)),
            )
            // v0.4.11 perf work: frame-time telemetry for the phone
            // log (registered here because this plugin is added on
            // both app paths — the stats are the whole frame, not
            // the HUD).
            .add_systems(
                Update,
                crate::perf::log_frame_time.run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                upload_ruffle_hud_frame.run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                update_ruffle_hud_visibility.run_if(in_state(AppState::InGame)),
            )
            // v0.4.9: exit on a forwarded wgpu device error — this
            // preserves Bevy's exit-on-render-error behaviour after
            // `install_wgpu_error_handlers` replaces its handlers
            // (see that system's docs). No state gate: a device loss
            // can happen in any state, including Loading.
            .add_systems(Update, exit_on_device_error);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .insert_resource(mailbox)
                .insert_resource(ExitRequested::default())
                .add_systems(ExtractSchedule, extract_ruffle_hud)
                .add_systems(ExtractSchedule, extract_app_exit)
                .add_systems(RenderStartup, install_wgpu_error_handlers)
                .add_systems(
                    Render,
                    ruffle_hud_render_system.in_set(RenderSystems::PrepareAssets),
                )
                .add_systems(
                    Render,
                    log_cluster_buffer_sizes.in_set(RenderSystems::Queue),
                )
                .add_systems(
                    Render,
                    drop_swapchain_texture_on_exit.in_set(RenderSystems::Cleanup),
                );
            // `SubApp` has no `insert_non_send_resource` — go
            // through the render world directly.
            render_app
                .world_mut()
                .insert_non_send(RuffleHudPlayer::default());
        }
    }
}

/// Read the HUD SWF + fonts from the game's data, synthesize the
/// fontlib, and store the prepared assets. Runs on the first InGame
/// entry; a no-op afterwards (the assets are per-app-run, not
/// per-scene). Any failure (missing files, bad fontlib) logs a
/// warning and leaves the assets absent — the Ruffle HUD stays off
/// and the Bevy HUD remains (the fallback).
fn prepare_ruffle_hud_assets(
    mut provider: ResMut<AssetProvider>,
    target: Option<Res<ScreenshotTarget>>,
    images: Res<Assets<Image>>,
    window: Query<&Window, With<PrimaryWindow>>,
    existing: Option<Res<RuffleHudAssets>>,
    mut commands: Commands,
) {
    if existing.is_some() {
        return;
    }
    let Some(hud_swf) = provider.read_file_latest(HUD_SWF_PATH) else {
        eprintln!(
            "warning: ruffle HUD: {HUD_SWF_PATH} not found in the game data — the Bevy HUD stays"
        );
        crate::startup_log::log(&format!(
            "ruffle HUD: FAILED — {HUD_SWF_PATH} not found in the game data"
        ));
        return;
    };
    let Some(fonts_swf) = provider.read_file_latest(FONTS_SWF_PATH) else {
        eprintln!(
            "warning: ruffle HUD: {FONTS_SWF_PATH} not found in the game data — the Bevy HUD stays"
        );
        crate::startup_log::log(&format!(
            "ruffle HUD: FAILED — {FONTS_SWF_PATH} not found in the game data"
        ));
        return;
    };
    let fontlib = match synthesize_fontlib(&fonts_swf) {
        Ok(fontlib) => fontlib,
        Err(e) => {
            eprintln!("warning: ruffle HUD: {e} — the Bevy HUD stays");
            crate::startup_log::log(&format!("ruffle HUD: FAILED — fontlib synthesis: {e}"));
            return;
        }
    };
    // The HUD size: the screenshot target's dimensions on the
    // screenshot path, the primary window's on the windowed path,
    // 1280×720 when neither exists — capped at the SWF's design
    // size per dimension.
    let target_size = target
        .as_ref()
        .and_then(|t| images.get(&t.image))
        .map(|image| (image.width(), image.height()));
    let window_size = window
        .single()
        .ok()
        .map(|w| (w.width() as u32, w.height() as u32));
    let (width, height) = hud_size_for(target_size, window_size);
    eprintln!(
        "ruffle HUD: prepared hudmenu.gfx ({} bytes) + fontlib ({} bytes) at {width}x{height}",
        hud_swf.len(),
        fontlib.len()
    );
    // The phone log captures ONLY `startup_log` lines (`eprintln!`
    // goes to /dev/null on Android) — v0.4.8 shipped with zero
    // Ruffle lines in the phone log, so the HUD's silence on the
    // phone was undiagnosable. Every Ruffle stage now logs here.
    crate::startup_log::log(&format!(
        "ruffle HUD: prepared hudmenu.gfx ({} bytes) + fontlib ({} bytes) at {width}x{height}",
        hud_swf.len(),
        fontlib.len()
    ));
    commands.insert_resource(RuffleHudAssets {
        width,
        height,
        hud_swf: Arc::new(hud_swf),
        fontlib: Arc::new(fontlib),
        base_path: provider.data_dir().to_path_buf(),
        safe_insets: (0, 0, 0, 0),
    });
}

/// The HUD render size from the available sources: the screenshot
/// target wins, then the window, then the 1280×720 default — each
/// dimension capped at the SWF's design size.
fn hud_size_for(target_size: Option<(u32, u32)>, window_size: Option<(u32, u32)>) -> (u32, u32) {
    let (w, h) = target_size
        .or(window_size)
        .unwrap_or((HUD_MAX_WIDTH, HUD_MAX_HEIGHT));
    (w.clamp(1, HUD_MAX_WIDTH), h.clamp(1, HUD_MAX_HEIGHT))
}

/// Compute the current cell's display name on every InGame entry
/// (it changes per scene — the assets do not).
fn update_current_location(
    cell: Option<Res<CellData>>,
    plugin: Option<Res<SharedPlugin>>,
    strings: Option<Res<GameStrings>>,
    mut commands: Commands,
) {
    let Some(cell) = cell else {
        return;
    };
    let name = cell_location_name(
        &cell,
        plugin.as_deref().map(|p| p.0.as_ref()),
        strings.as_deref(),
    );
    commands.insert_resource(CurrentLocation(name));
}

/// The HUD location name for a cell: the interior CELL record's
/// FULL (via `.strings`, falling back to the prettified EDID), or
/// "Riverwood" for the exterior (the only exterior block is
/// Riverwood 5,-11 — a proper LCTN lookup is future work).
fn cell_location_name(
    cell: &CellData,
    plugin: Option<&dovah_esm::Plugin>,
    strings: Option<&GameStrings>,
) -> String {
    if !cell.interior {
        return "Riverwood".to_string();
    }
    if let (Some(plugin), Some(strings)) = (plugin, strings) {
        if let Some(record) = plugin.find(cell.form_id) {
            if let Some(name) = strings.name_of(record) {
                return name;
            }
        }
    }
    prettify_edid(&cell.edid)
}

/// A base record's display name: its FULL from `.strings` when the
/// plugin + strings are available and the record has one, else the
/// prettified EDID. (The DOOR record's FULL is NOT the destination
/// name for a load door — the inn door's base is a generic "Wooden
/// Door"; load doors resolve through their `DoorTrigger` target,
/// below. Room doors have no trigger and correctly show the door
/// record's FULL, which is what the real game shows for them.)
fn record_display_name(
    plugin: Option<&dovah_esm::Plugin>,
    strings: Option<&GameStrings>,
    base_form_id: u32,
    base_edid: &str,
) -> String {
    if let (Some(plugin), Some(strings)) = (plugin, strings) {
        if let Some(record) = plugin.find(base_form_id) {
            if let Some(name) = strings.name_of(record) {
                return name;
            }
        }
    }
    prettify_edid(base_edid)
}

/// A load door's destination display name: the destination CELL's
/// FULL for an interior target (the same lookup as
/// `cell_location_name`, by the form ID the `SceneTarget` now
/// carries), "Riverwood" for an exterior target (the only exterior
/// block the engine knows — the same M4 value as the location
/// name). Falls back to the prettified destination EDID when the
/// plugin/strings cannot resolve the FULL (or the form ID is 0,
/// the `--cell` unknown marker).
fn door_destination_name(
    target: &SceneTarget,
    plugin: Option<&dovah_esm::Plugin>,
    strings: Option<&GameStrings>,
) -> String {
    match target {
        SceneTarget::Interior { form_id, edid } => {
            if *form_id != 0 {
                if let (Some(plugin), Some(strings)) = (plugin, strings) {
                    if let Some(record) = plugin.find(*form_id) {
                        if let Some(name) = strings.name_of(record) {
                            return name;
                        }
                    }
                }
            }
            prettify_edid(edid)
        }
        SceneTarget::Exterior { .. } => "Riverwood".to_string(),
    }
}

/// The crosshair resolution inputs, bundled so the pure resolver
/// stays testable without a Bevy world.
struct CrosshairInputs<'a> {
    doors: &'a Doors,
    talkable: &'a TalkableNpcs,
    pickupables: &'a Pickupables,
    containers: &'a Containers,
    cell_objects: &'a [PlacedObject],
    /// Load-door triggers (`CellData.doors`): a door in range
    /// whose ref is here is a load door, and its `target` names
    /// the destination the real game shows. A door with no
    /// trigger is a room door.
    door_targets: &'a [DoorTrigger],
    plugin: Option<&'a dovah_esm::Plugin>,
    strings: Option<&'a GameStrings>,
    names: &'a HashMap<u32, String>,
    defs: &'a HashMap<u32, ItemDef>,
}

/// Nearest door in USE range AND in the player's forward cone
/// (mirrors `nearest_door_in_range`'s range rule — horizontal
/// distance to the ref — plus the facing check). Pure.
fn nearest_door_in_range_facing(doors: &Doors, feet: Vec3, forward: Vec3) -> Option<u32> {
    let mut best: Option<(u32, f32)> = None;
    for (ref_form_id, door) in &doors.doors {
        let dx = feet.x - door.plane_point.x;
        let dz = feet.z - door.plane_point.z;
        let dist = (dx * dx + dz * dz).sqrt();
        if dist > USE_RANGE_M {
            continue;
        }
        if !is_facing_target(feet, forward, door.plane_point) {
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

/// Nearest pickupable in USE range AND in the player's forward
/// cone (mirrors `nearest_pickupable`'s range rule plus the
/// facing check). Pure.
fn nearest_pickupable_facing(items: &Pickupables, feet: Vec3, forward: Vec3) -> Option<u32> {
    let mut best: Option<(u32, f32)> = None;
    for (ref_form_id, item) in &items.items {
        if !in_use_range(item.position, feet) {
            continue;
        }
        if !is_facing_target(feet, forward, item.position) {
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

/// Nearest container in USE range AND in the player's forward
/// cone (mirrors `nearest_container`'s range rule plus the
/// facing check). Pure.
fn nearest_container_facing(items: &Containers, feet: Vec3, forward: Vec3) -> Option<u32> {
    let mut best: Option<(u32, f32)> = None;
    for (ref_form_id, item) in &items.items {
        if !in_use_range(item.position, feet) {
            continue;
        }
        if !is_facing_target(feet, forward, item.position) {
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

/// Resolve the crosshair target at the player's feet, in the USE
/// priority order (lib.rs): door > dialogue (NPC) > pickup >
/// container. A candidate must be in range AND faced (v0.4.13:
/// the crosshair named Embry while Dylan aimed at the fire pit
/// — proximity alone is not a target). A candidate that cannot
/// be named (a pickup whose def is missing and has no harvest
/// name) falls through to the next priority instead of showing
/// a blank target.
///
/// The name carries the verb — the player sees what USE will
/// do, not just the name (v0.4.13: "no interaction boxes").
/// Doors "Open <name>" and containers "Search <name>" are
/// vanilla-exact; pickups "Take <name>" likewise; NPCs "Talk to
/// <name>" is the phone-appropriate form (vanilla shows the bare
/// name beside the E keycap, and there is no E key on a phone).
fn resolve_crosshair(inputs: &CrosshairInputs, feet: Vec3, forward: Vec3) -> CrosshairTarget {
    // Door: a load door shows its DESTINATION (the real game's
    // behavior — the inn door prompts "Sleeping Giant Inn", not
    // the generic "Wooden Door" of its base record); a room door
    // (no trigger) shows the door record's FULL, which is what
    // the real game shows for room doors.
    if let Some(ref_id) = nearest_door_in_range_facing(inputs.doors, feet, forward) {
        if let Some(obj) = inputs.cell_objects.iter().find(|o| o.ref_form_id == ref_id) {
            let name = match inputs.door_targets.iter().find(|t| t.ref_form_id == ref_id) {
                Some(trigger) => {
                    door_destination_name(&trigger.target, inputs.plugin, inputs.strings)
                }
                None => record_display_name(
                    inputs.plugin,
                    inputs.strings,
                    obj.base_form_id,
                    &obj.base_edid,
                ),
            };
            return CrosshairTarget::Target {
                name: format!("Open {name}"),
                weight: None,
                cost: None,
            };
        }
        // The door is in range but its PlacedObject is gone (a
        // despawned door?) — fall through rather than showing a
        // blank name.
    }
    // NPC: the dialogue index's name, else the prettified EDID.
    if let Some(ref_id) = nearest_npc_in_range_facing(inputs.talkable, feet, forward) {
        if let Some(npc) = inputs.talkable.npcs.get(&ref_id) {
            let name = inputs
                .names
                .get(&npc.base_form_id)
                .cloned()
                .unwrap_or_else(|| prettify_edid(&npc.base_edid));
            return CrosshairTarget::Target {
                name: format!("Talk to {name}"),
                weight: None,
                cost: None,
            };
        }
    }
    // Pickup: the harvest name (FLOR) or the catalog def's
    // name/weight/value. A missing def with no harvest name (an
    // LVLI yield) falls through.
    if let Some(ref_id) = nearest_pickupable_facing(inputs.pickupables, feet, forward) {
        if let Some(item) = inputs.pickupables.items.get(&ref_id) {
            if let Some(name) = &item.harvest_name {
                return CrosshairTarget::Target {
                    name: format!("Take {name}"),
                    weight: None,
                    cost: None,
                };
            }
            if let Some(def) = inputs.defs.get(&item.base_form_id) {
                return CrosshairTarget::Target {
                    name: format!("Take {}", def.name),
                    weight: Some(def.weight),
                    cost: Some(def.value),
                };
            }
        }
    }
    // Container: the CONT base record's FULL.
    if let Some(ref_id) = nearest_container_facing(inputs.containers, feet, forward) {
        if let Some(obj) = inputs.cell_objects.iter().find(|o| o.ref_form_id == ref_id) {
            return CrosshairTarget::Target {
                name: format!(
                    "Search {}",
                    record_display_name(
                        inputs.plugin,
                        inputs.strings,
                        obj.base_form_id,
                        &obj.base_edid,
                    )
                ),
                weight: None,
                cost: None,
            };
        }
    }
    CrosshairTarget::None
}

/// The drive system's read-only lookups, bundled as a SystemParam
/// (Bevy caps a system at 16 parameters).
#[derive(SystemParam)]
struct HudDriveParams<'w> {
    doors: Option<Res<'w, Doors>>,
    talkable: Option<Res<'w, TalkableNpcs>>,
    pickupables: Option<Res<'w, Pickupables>>,
    containers: Option<Res<'w, Containers>>,
    cell: Option<Res<'w, CellData>>,
    plugin: Option<Res<'w, SharedPlugin>>,
    strings: Option<Res<'w, GameStrings>>,
    dialogue_index: Option<Res<'w, DialogueIndex>>,
    catalog: Option<Res<'w, ItemCatalog>>,
    location: Option<Res<'w, CurrentLocation>>,
    settings: Option<Res<'w, GameSettings>>,
    cli_override: Option<Res<'w, RuffleHudCliOverride>>,
    dialogue_open: Option<Res<'w, crate::dialogue::DialogueOpen>>,
}

/// Snapshot live game state into `HudState` every InGame frame.
///
/// Sources (the same sources of truth the Bevy HUD and the save
/// system read): health/magicka are always 100.0 (no combat/magic
/// systems yet — the same hardcode as `SaveCaptureParams` and
/// `update_bars`); stamina is the `Stamina` resource (100.0 when
/// absent); the heading is `heading_degrees` of the player's yaw
/// (the compass fix's math); the location is the `CurrentLocation`
/// resource; the crosshair resolves from the interact indexes.
/// `enabled` is the CLI override when present (screenshot path),
/// else the `ruffle_hud` setting (default true when the setting
/// resource is absent).
fn drive_ruffle_hud_state(
    time: Res<Time>,
    stamina: Option<Res<Stamina>>,
    player: Query<(&Transform, &LookAngles), With<Player>>,
    params: HudDriveParams,
    mut state: ResMut<HudState>,
    mut logged_enabled: Local<bool>,
) {
    let Ok((transform, angles)) = player.single() else {
        return;
    };
    let feet = transform.translation - Vec3::new(0.0, EYE_HEIGHT, 0.0);
    let forward = *transform.forward();
    let (enabled, source) = if let Some(ov) = params.cli_override.as_deref() {
        (ov.0, "CLI override")
    } else {
        (
            params
                .settings
                .as_deref()
                .map(|s| s.ruffle_hud)
                .unwrap_or(true),
            "settings",
        )
    };
    // Empty defaults when the indexes are absent (the headless
    // regression test has no InteractPlugin/DialoguePlugin).
    let empty_doors = Doors::default();
    let empty_talkable = TalkableNpcs::default();
    let empty_pickupables = Pickupables::default();
    let empty_containers = Containers::default();
    let empty_names: HashMap<u32, String> = HashMap::new();
    let inputs = CrosshairInputs {
        doors: params.doors.as_deref().unwrap_or(&empty_doors),
        talkable: params.talkable.as_deref().unwrap_or(&empty_talkable),
        pickupables: params.pickupables.as_deref().unwrap_or(&empty_pickupables),
        containers: params.containers.as_deref().unwrap_or(&empty_containers),
        cell_objects: params
            .cell
            .as_deref()
            .map(|c| c.objects.as_slice())
            .unwrap_or(&[]),
        door_targets: params
            .cell
            .as_deref()
            .map(|c| c.doors.as_slice())
            .unwrap_or(&[]),
        plugin: params.plugin.as_deref().map(|p| p.0.as_ref()),
        strings: params.strings.as_deref(),
        names: params
            .dialogue_index
            .as_deref()
            .map(|d| &d.names)
            .unwrap_or(&empty_names),
        defs: params
            .catalog
            .as_deref()
            .map(|c| &c.defs)
            .unwrap_or(&*EMPTY_DEFS),
    };
    state.health = 100.0;
    state.magicka = 100.0;
    state.stamina = stamina.as_deref().map(|s| s.value).unwrap_or(100.0);
    state.heading_deg = heading_degrees(angles.yaw);
    state.location = params
        .location
        .as_deref()
        .map(|l| l.0.clone())
        .unwrap_or_default();
    // No crosshair target while a dialogue is open: vanilla hides
    // the crosshair in dialogue, and the Ruffle name double-drew
    // with the dialogue's gold speaker name (v0.4.13 shots 03/04).
    // The change detection in `drive_hud_from_state` fires the
    // deactivate call once, and re-resolves on close.
    state.crosshair = if params.dialogue_open.map(|o| o.0).unwrap_or(false) {
        CrosshairTarget::None
    } else {
        resolve_crosshair(&inputs, feet, forward)
    };
    state.hint = None;
    state.enabled = enabled;
    state.delta_secs = time.delta_secs();
    // One-time: the resolved enable flag is the first thing to
    // check when the Ruffle HUD never appears (the render system
    // pauses stepping while it is false).
    if !*logged_enabled {
        *logged_enabled = true;
        crate::startup_log::log(&format!("ruffle HUD: enabled={enabled} (from {source})"));
    }
}

/// An empty item-def map for the drive system when the catalog
/// resource is absent. (A `static` so the drive does not allocate
/// a fresh empty map every frame.)
static EMPTY_DEFS: std::sync::LazyLock<HashMap<u32, ItemDef>> =
    std::sync::LazyLock::new(HashMap::new);

/// Take the latest captured HUD frame from the mailbox and write it
/// into the Bevy `Image` asset (creating the asset + the fullscreen
/// UI node on the first frame). The image is `Rgba8Unorm`, NOT
/// sRGB: Ruffle renders in sRGB space with a non-sRGB target, so a
/// second sRGB conversion would double-convert.
fn upload_ruffle_hud_frame(
    mailbox: Res<RuffleHudMailbox>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
    texture: Option<Res<RuffleHudTexture>>,
    mut active: ResMut<RuffleHudActive>,
) {
    let Some(frame) = mailbox.0.lock().expect("mailbox lock").take() else {
        return;
    };
    let (width, height) = (frame.width(), frame.height());
    let pixels = frame.into_raw();
    match texture {
        Some(texture) => {
            if let Some(mut image) = images.get_mut(&texture.handle) {
                image.data = Some(pixels);
            }
        }
        None => {
            let image = Image::new(
                Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                pixels,
                TextureFormat::Rgba8Unorm,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            );
            let handle = images.add(image);
            commands.spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    position_type: bevy::ui::PositionType::Absolute,
                    ..Default::default()
                },
                ImageNode::new(handle.clone()),
                // The HUD must not eat taps: the pause/inventory/
                // dialogue menus use PointerClick observers, and a
                // fullscreen UI node without `Pickable` blocks
                // picking by default. It is a passive display, so
                // it is not hoverable either (no events of its own).
                Pickable {
                    should_block_lower: false,
                    is_hoverable: false,
                },
                RuffleHudRoot,
            ));
            commands.insert_resource(RuffleHudTexture { handle });
            // One-time: the first upload proves the whole pipeline
            // (prepare → build → settle → capture → mailbox).
            crate::startup_log::log(&format!(
                "ruffle HUD: first frame uploaded to Bevy image ({width}x{height})"
            ));
        }
    }
    active.0 = true;
}

/// Show the Ruffle HUD node and hide the Bevy display elements
/// while the Ruffle HUD is enabled AND has delivered a frame; show
/// the Bevy display otherwise. Only writes on change.
fn update_ruffle_hud_visibility(
    state: Res<HudState>,
    active: Res<RuffleHudActive>,
    // B0001: two `&mut Visibility` queries are a param conflict
    // unless Bevy can prove them disjoint — the `With` filters
    // alone do not prove it (an entity could carry both
    // markers), so each query also excludes the other's marker.
    // (No entity should carry both; the exclusion is belt and
    // braces, and the system would panic at init — even
    // run_if-gated — without it.)
    mut ruffle_nodes: Query<&mut Visibility, (With<RuffleHudRoot>, Without<BevyHudDisplay>)>,
    mut bevy_nodes: Query<&mut Visibility, (With<BevyHudDisplay>, Without<RuffleHudRoot>)>,
    mut last: Local<Option<bool>>,
) {
    let ruffle_visible = state.enabled && active.0;
    if *last == Some(ruffle_visible) {
        return;
    }
    *last = Some(ruffle_visible);
    let ruffle_visibility = if ruffle_visible {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    let bevy_visibility = if ruffle_visible {
        Visibility::Hidden
    } else {
        Visibility::Visible
    };
    for mut visibility in &mut ruffle_nodes {
        *visibility = ruffle_visibility;
    }
    for mut visibility in &mut bevy_nodes {
        *visibility = bevy_visibility;
    }
}

/// Copy the main-world HUD state + assets into the render world
/// every frame. (`Res`/`Query` params read the MAIN world in an
/// extract system; `Commands` write to the RENDER world.)
/// Copy the main world's HUD state + prepared assets into the
/// render world. `Extract<...>` is load-bearing: extract systems
/// run on the RENDER world, and a plain `Res<T>` reads the render
/// world's copy (which does not exist) — only `Extract<Res<T>>`
/// reads the MAIN world (bevy_extract's `MainWorld` swap). The
/// first M4 build used plain `Res` and the render system never
/// saw the assets (no error, no player, no HUD).
fn extract_ruffle_hud(
    mut commands: Commands,
    state: Extract<Option<Res<HudState>>>,
    assets: Extract<Option<Res<RuffleHudAssets>>>,
) {
    if let Some(state) = state.as_deref() {
        commands.insert_resource(state.clone());
    }
    if let Some(assets) = assets.as_deref() {
        commands.insert_resource(assets.clone());
    }
}

/// Whether an `AppExit` was requested (v0.4.9 teardown fix).
/// Set by [`extract_app_exit`] in the exit update; read by
/// [`drop_swapchain_texture_on_exit`] in the same update's
/// `Cleanup` phase. Latched — once set it stays set, which is
/// safe: the app exits at the end of the update, and if it
/// somehow did not, the next frame re-acquires a texture.
#[derive(Resource, Default)]
struct ExitRequested(bool);

/// Copy the main world's `AppExit` request into the render world
/// (v0.4.9 teardown fix). Runs in the `ExtractSchedule`, which
/// always runs in the same update that wrote the `AppExit` —
/// `AppExit` is only ever written by a system (pause Quit,
/// `exit_on_all_closed`), and every update runs the main schedule
/// then extract then the render schedule.
fn extract_app_exit(
    mut exit: Extract<MessageReader<AppExit>>,
    mut requested: ResMut<ExitRequested>,
) {
    if exit.read().next().is_some() {
        requested.0 = true;
    }
}

/// Drop the live swapchain texture on the exit update (v0.4.9
/// teardown fix) — or the app panics at teardown with
/// `Trying to destroy a SwapchainAcquireSemaphore that is still
/// in use by a SurfaceTexture` (wgpu-hal, seen on the v0.4.8
/// phone build at +110s).
///
/// The mechanism, proven from source (bevy_render 0.20.1,
/// bevy_ecs 0.20.1, wgpu-hal 30.0.2):
/// - A `SurfaceTexture` holds a clone of the swapchain's acquire
///   semaphore (`wgpu-hal .../vulkan/swapchain/native.rs`, in
///   `acquire()`). `release_resources()` destroys each semaphore
///   with `Arc::into_inner(...).expect(...)` — it PANICS if a
///   `SurfaceTexture` is still alive when the `Surface` drops.
/// - The only live `SurfaceTexture` in Bevy is
///   `ExtractedWindow.swap_chain_texture`. It is acquired in
///   `prepare_windows` and normally presented (taken) by
///   `render_system` — but present is skipped when no view
///   rendered to the window that frame (`view_needs_present`
///   false), leaving the texture alive into teardown.
/// - No ECS removal can fix the order: table columns drop in
///   ascending `ComponentId` order (bevy_ecs asserts the
///   columns are sorted, and both `Table::drop` and
///   `move_row` iterate in that order) — and in the render
///   world `SurfaceData`'s id is lower than
///   `ExtractedWindow`'s, so the surface always drops FIRST,
///   whether via Bevy's `WindowClosing` removal or the render
///   world's drop. The v0.4.8 backtrace shows the world drop.
/// - So the texture must be dropped EXPLICITLY, after the last
///   present, before the `App` is dropped. `RenderSystems::
///   Cleanup` runs after `Render` (where present happens) in
///   the same update — the last chance before the loop exits
///   and `App::run()` drops the render world.
///
/// Ruffle is NOT involved: the Ruffle player renders to its own
/// offscreen `TextureTarget` and never touches the swapchain.
/// This lives in `RuffleHudPlugin` only because it is the
/// existing plugin with render-app access.
fn drop_swapchain_texture_on_exit(
    requested: Res<ExitRequested>,
    mut windows: Query<&mut ExtractedWindow>,
) {
    if !requested.0 {
        return;
    }
    for mut window in windows.iter_mut() {
        // Dropping a `SurfaceTexture` without presenting just
        // releases it back to the swapchain — the semaphore Arc
        // goes with it, so `release_resources()` stops panicking.
        // v0.4.9: log when the fix actually drops a live texture —
        // the v0.4.9 phone log could not prove the fix was exercised
        // (a clean exit also happens when no texture is live).
        if window.swap_chain_texture.take().is_some() {
            crate::startup_log::log("teardown fix: dropped live swapchain texture on exit");
        }
    }
}

/// Whether a wgpu device error (uncaptured validation error or
/// device loss) has been reported by the forwarded handlers (see
/// `install_wgpu_error_handlers`). Set from the render thread,
/// read by `exit_on_device_error` on the main thread.
static DEVICE_ERROR: AtomicBool = AtomicBool::new(false);

/// Forward wgpu errors to the startup log (v0.4.9 diagnosis).
///
/// Bevy installs its own uncaptured-error handler + device-lost
/// callback (`DeviceErrorHandler`, bevy_render), both logging via
/// tracing — INVISIBLE in the phone log. The v0.4.9 cluster-buffer
/// panic's underlying wgpu error was therefore lost: the log shows
/// the panic but not the reason the buffer creation failed (device
/// lost vs out of memory vs validation). These replacements log
/// the error text to the startup log AND preserve Bevy's exit
/// behaviour: Bevy's default `RenderErrorHandler` writes
/// `AppExit::error()` for any render error, so the handlers set
/// `DEVICE_ERROR` and `exit_on_device_error` (main world, every
/// frame) writes the same exit. Without the flag, replacing
/// Bevy's handlers would turn a device loss into a hang (the
/// render would keep failing on a lost device) instead of a
/// clean exit.
///
/// Runs once in `RenderStartup` — after `RenderPlugin` built the
/// device and installed Bevy's handlers, so these replace them.
/// Ruffle installs no error handler of its own (checked
/// 2026-11-16), so nothing replaces these afterwards.
fn install_wgpu_error_handlers(render_device: Res<RenderDevice>) {
    let device = render_device.wgpu_device().clone();
    device.on_uncaptured_error(Arc::new(|error| {
        crate::startup_log::log(&format!("wgpu error: {error}"));
        DEVICE_ERROR.store(true, Ordering::SeqCst);
    }));
    device.set_device_lost_callback(|reason, message| {
        crate::startup_log::log(&format!("wgpu device lost: {reason:?} — {message}"));
        DEVICE_ERROR.store(true, Ordering::SeqCst);
    });
    crate::startup_log::log("wgpu error handlers installed (forwarding to startup log)");
}

/// Preserve Bevy's exit-on-render-error behaviour after
/// `install_wgpu_error_handlers` replaced its handlers (see that
/// system's docs). Bevy's default `RenderErrorHandler` writes
/// `AppExit::error()` for any render error; this does the same
/// when the forwarded handler fires.
fn exit_on_device_error(mut exit: MessageWriter<AppExit>) {
    if DEVICE_ERROR.swap(false, Ordering::SeqCst) {
        crate::startup_log::log("exiting on wgpu device error (AppExit::error)");
        exit.write(AppExit::error());
    }
}

/// Per-frame state for `log_cluster_buffer_sizes`.
#[derive(Default)]
struct ClusterDiagState {
    /// Whether the one-time adapter/limits line was logged.
    limits_logged: bool,
    /// Frames since the last sizes line (throttle).
    frames_since_log: u32,
    /// Last logged (total indices, total offsets) — the sizes line
    /// only fires when the counts change.
    last_counts: Option<(usize, usize)>,
}

/// Log the cluster-buffer sizes + device limits to the startup
/// log (v0.4.9 diagnosis).
///
/// The v0.4.9 panic was `create_buffer_init` failing for the
/// cluster index-lists buffer. The size is bounded by
/// construction (default fixed-Z clustering: at most 4096
/// clusters per view, so 8 lights × 4096 = 32,768 indices =
/// 128 KB — far under any Adreno `max_buffer_size`), so a small
/// logged size + a panic points at device loss / OOM, not size.
/// This system logs the adapter + limits once, then the live
/// counts (indices + offsets across all views, with the implied
/// byte sizes) whenever they change (throttled to one line per
/// 60 frames) — so the last line before a panic is the size that
/// was being requested.
///
/// Runs in `RenderSystems::Queue` — after `Prepare`, so the
/// counts are the current frame's (prepare_clusters rebuilds the
/// bindings every frame).
fn log_cluster_buffer_sizes(
    views: Query<&ViewClusterBindings>,
    render_device: Res<RenderDevice>,
    mut state: Local<ClusterDiagState>,
) {
    if !state.limits_logged {
        state.limits_logged = true;
        let info = render_device.wgpu_device().adapter_info();
        crate::startup_log::log(&format!(
            "cluster diag: adapter {} ({:?}), max_buffer_size {} MB",
            info.name,
            info.backend,
            render_device.limits().max_buffer_size / (1024 * 1024),
        ));
    }
    state.frames_since_log += 1;
    let total_indices: usize = views.iter().map(|b| b.n_indices()).sum();
    let total_offsets: usize = views.iter().map(|b| b.n_offsets()).sum();
    let counts = (total_indices, total_offsets);
    // The first line fires immediately (at ~1 fps the 60-frame
    // throttle would delay it past the v0.4.9 panic point);
    // afterwards only on change, throttled to one line per 60
    // frames.
    let first = state.last_counts.is_none();
    if first || (state.last_counts != Some(counts) && state.frames_since_log >= 60) {
        state.frames_since_log = 0;
        state.last_counts = Some(counts);
        // Index lists: one u32 per index. Offsets/counts: two
        // UVec4s (32 bytes) per offset.
        crate::startup_log::log(&format!(
            "cluster diag: {} views, {} indices ({} KB index lists), {} offsets ({} KB offsets/counts)",
            views.iter().count(),
            total_indices,
            total_indices * 4 / 1024,
            total_offsets,
            total_offsets * 32 / 1024,
        ));
    }
}

/// v0.4.11 perf work: how many movie steps this render frame may
/// take, and the accumulator afterwards. At most ONE step per
/// frame — the old 3-step catch-up cap created a spiral (below
/// 30 fps the movie clock falls behind by more than 3 frames every
/// frame, so the engine paid 3x run+render forever; measured on
/// desktop at exactly 3.00 steps per render frame). The backlog is
/// clamped to one movie frame so a slow game cannot accumulate an
/// unbounded debt; the movie then runs at the render rate when the
/// game is slow. Returns `(steps, new_accumulator)`.
fn movie_steps_due(time_accum: f32, delta_secs: f32, force_first: bool) -> (u32, f32) {
    let mut accum = time_accum + delta_secs;
    let clock_due = accum >= MOVIE_FRAME_SECS;
    let mut steps = 0;
    if clock_due || force_first {
        if clock_due {
            accum -= MOVIE_FRAME_SECS;
        }
        steps = 1;
    }
    (steps, accum.min(MOVIE_FRAME_SECS))
}

/// The render-world Ruffle system (M4): build the player lazily on
/// Bevy's device, drive it from the extracted state, step the movie
/// at 30 fps, and capture frames into the mailbox.
///
/// Runs in `RenderSystems::PrepareAssets` — early, before the
/// render graph submits — because `capture_frame` blocks on
/// `device.poll(Wait)` for ALL submitted work; running before the
/// graph submits avoids stalling on Bevy's frame.
#[allow(clippy::too_many_arguments)]
fn ruffle_hud_render_system(
    mut player: NonSendMut<RuffleHudPlayer>,
    assets: Option<Res<RuffleHudAssets>>,
    state: Option<Res<HudState>>,
    mailbox: Res<RuffleHudMailbox>,
    render_device: Option<Res<RenderDevice>>,
    render_queue: Option<Res<RenderQueue>>,
    render_adapter: Option<Res<RenderAdapter>>,
    render_instance: Option<Res<RenderInstance>>,
) {
    let (Some(assets), Some(state)) = (assets.as_deref(), state.as_deref()) else {
        return;
    };
    // Destructure so the harness borrow does not conflict with the
    // drive-state fields (one `NonSendMut` cannot lend two mutable
    // borrows at once).
    let RuffleHudPlayer {
        harness,
        last_driven,
        settled_frames,
        initialized,
        time_accum,
        first_frame_logged,
        slow_step_logged,
        capture_none_logged,
        ruffle_stats,
    } = &mut *player;
    // Build the player on first use (slow: ~1–3 s of shader
    // compile + fontlib import — once, on the first InGame frame).
    if harness.is_none() {
        let (Some(device), Some(queue), Some(adapter), Some(instance)) = (
            render_device.as_deref(),
            render_queue.as_deref(),
            render_adapter.as_deref(),
            render_instance.as_deref(),
        ) else {
            return;
        };
        let shared = SharedWgpu {
            instance: (**instance).clone(),
            adapter: (**adapter).clone(),
            device: device.wgpu_device().clone(),
            queue: (**queue).clone(),
        };
        match Harness::new_shared(
            shared,
            &assets.hud_swf,
            &assets.fontlib,
            "hudmenu.gfx",
            assets.base_path.clone(),
            assets.width,
            assets.height,
            30.0,
        ) {
            Ok(mut built) => {
                built.set_transparent();
                let (left, top, right, bottom) = assets.safe_insets;
                built.set_safe_area_insets(left, top, right, bottom);
                *harness = Some(built);
                eprintln!(
                    "ruffle HUD: player built on the shared device ({}x{})",
                    assets.width, assets.height
                );
                crate::startup_log::log(&format!(
                    "ruffle HUD: player built on the shared device ({}x{})",
                    assets.width, assets.height
                ));
            }
            Err(e) => {
                eprintln!("warning: ruffle HUD: player build failed ({e}) — the Bevy HUD stays");
                crate::startup_log::log(&format!("ruffle HUD: FAILED — player build: {e}"));
                return;
            }
        }
    }
    let Some(harness) = harness.as_mut() else {
        return;
    };
    // Paused while disabled (the setting toggled off, or the
    // screenshot path without the flag): the mailbox goes stale and
    // the visibility system hides the node. The player stays built,
    // so re-enabling resumes immediately.
    if !state.enabled {
        return;
    }
    // Settle: step frames until the movie's frame-1 scripts have
    // run and the fontlib import has completed, then run
    // `InitExtensions` (it registers every GameDelegate callback)
    // and drive the initial state. v0.4.9: the settle uses
    // `step_logic` (no render) — settle frames are never
    // captured, so rasterizing them is wasted GPU work, and on
    // the phone each full step may cost ~800 ms (the v0.4.9 HUD
    // never reached its first capture in 8.4 s — see
    // docs/v049-crash-diagnosis.md).
    if !*initialized {
        let timing = harness.step_logic();
        *settled_frames += 1;
        // One-time: log the first settle step's cost — the phone's
        // per-step cost is the open question from v0.4.9.
        if *settled_frames == 1 {
            crate::startup_log::log(&format!(
                "ruffle HUD: settle step 1 (preload+run={:.1}ms)",
                timing.preload_run_ms
            ));
        }
        if timing.preload_run_ms > 100.0 && !*slow_step_logged {
            *slow_step_logged = true;
            crate::startup_log::log(&format!(
                "ruffle HUD: slow settle step ({:.1}ms preload+run)",
                timing.preload_run_ms
            ));
        }
        if *settled_frames >= SETTLE_FRAMES {
            harness.call_root_function("InitExtensions", vec![]);
            drive_hud_from_state(harness, state, last_driven);
            *initialized = true;
            crate::startup_log::log("ruffle HUD: settled (10 frames), InitExtensions called");
        }
        return;
    }
    // Drive the changed values, then step the movie on its 30 fps
    // clock and capture the frame. The drive runs every render
    // frame (the SWF just records the latest values).
    // v0.4.11 perf work: ONE movie step per render frame, and the
    // accumulator is clamped to one frame of backlog. The old
    // 3-step catch-up cap created a spiral: below 30 fps the movie
    // clock falls behind by more than 3 frames every frame, so the
    // engine paid 3x run+render forever (measured on desktop:
    // exactly 3.00 steps per render frame, 666 ms of Ruffle work
    // per frame; the phone's first-step render alone ran 15-121 ms
    // across the v0.4.10 logs). With one step the movie runs at the
    // render rate when the game is slow — its frame-counted fades
    // take longer in real time, but the HUD stays correct: the
    // driven state (compass, bars, crosshair) is applied fresh
    // every frame, so nothing is ever stale by more than one frame.
    // At 30+ fps nothing changes (the clock is due at most once
    // per render frame).
    // v0.4.9: the FIRST post-settle step is forced regardless of
    // the clock — if the clock gate is the blocker, the HUD must
    // still appear, and the first-frame log must still fire.
    drive_hud_from_state(harness, state, last_driven);
    let force_first = !*first_frame_logged;
    let (steps, new_accum) = movie_steps_due(*time_accum, state.delta_secs, force_first);
    *time_accum = new_accum;
    let mut stepped = 0;
    let mut last_timing = None;
    if steps > 0 {
        let timing = harness.step();
        ruffle_stats.record_step(&timing);
        last_timing = Some(timing);
        stepped += 1;
    }
    if stepped > 0 {
        // One-time: log the first post-settle step's clock + cost.
        if !*first_frame_logged {
            if let Some(t) = &last_timing {
                crate::startup_log::log(&format!(
                    "ruffle HUD: first movie step (delta={:.3}s, accum={:.3}s, preload+run={:.1}ms, render={:.1}ms)",
                    state.delta_secs,
                    *time_accum,
                    t.preload_run_ms,
                    t.render_ms
                ));
            }
        }
        if let Some(t) = &last_timing {
            let total = t.preload_run_ms + t.render_ms;
            if total > 100.0 && !*slow_step_logged {
                *slow_step_logged = true;
                crate::startup_log::log(&format!(
                    "ruffle HUD: slow movie step ({total:.1}ms: preload+run={:.1}ms, render={:.1}ms)",
                    t.preload_run_ms,
                    t.render_ms
                ));
            }
        }
        // v0.4.11 perf work: time the capture — the readback is a
        // blocking GPU sync (`map_async` + `device.poll(Wait)` +
        // row copy + alpha unmultiply), and its steady-state cost
        // on the phone is unmeasured. The stats line reports the
        // per-window average/max.
        let capture_started = Instant::now();
        let captured = harness.capture();
        ruffle_stats.record_capture(capture_started.elapsed().as_secs_f64() * 1000.0);
        match captured {
            Some(frame) => {
                if !*first_frame_logged {
                    *first_frame_logged = true;
                    crate::startup_log::log(&format!(
                        "ruffle HUD: first frame captured ({}x{})",
                        frame.width(),
                        frame.height()
                    ));
                }
                *mailbox.0.lock().expect("mailbox lock") = Some(frame);
            }
            // v0.4.9: capture returning `None` was silent — log it
            // (once). It should never happen for this target
            // (`capture_frame` returns `Some` whenever the staging
            // buffer exists, which it always does); a `None` here
            // would mean the renderer's downcast failed.
            None => {
                if !*capture_none_logged {
                    *capture_none_logged = true;
                    crate::startup_log::log("ruffle HUD: capture returned None after step");
                }
            }
        }
    }
    // v0.4.11 perf work: flush the Ruffle cost window when due
    // (runs every render frame that reaches here, stepped or not,
    // so the line still fires on frames with no movie step).
    if let Some(line) = ruffle_stats.flush(Instant::now()) {
        crate::startup_log::log(&line);
    }
}

/// Drive the SWF's GameDelegate handlers from the snapshot, with
/// change detection against the last driven state. Meters fire on
/// change (epsilon 0.05 — the fade-in anim restarts on every call,
/// so spamming it would never finish); the compass fires every
/// frame (a direct `_x` assignment, idempotent); the location
/// fires on change (`SetLocationName` restarts its fade-in);
/// the crosshair fires on change; the hint fires on change (a
/// `None` hint after a `Some` hint hides it).
fn drive_hud_from_state(harness: &mut Harness, state: &HudState, last: &mut Option<HudState>) {
    let changed = |f: fn(&HudState) -> f32, eps: f32| -> bool {
        match last.as_ref() {
            None => true,
            Some(prev) => (f(state) - f(prev)).abs() > eps,
        }
    };
    if changed(|s| s.health, 0.05) {
        harness.call(
            "SetHealthMeterPercent",
            vec![Value::Number(state.health as f64), Value::Bool(false)],
        );
    }
    if changed(|s| s.magicka, 0.05) {
        harness.call(
            "SetMagickaMeterPercent",
            vec![Value::Number(state.magicka as f64), Value::Bool(false)],
        );
    }
    if changed(|s| s.stamina, 0.05) {
        harness.call(
            "SetStaminaMeterPercent",
            vec![Value::Number(state.stamina as f64), Value::Bool(false)],
        );
    }
    // Compass: every frame (the player turns continuously).
    harness.call(
        "SetCompassAngle",
        vec![
            Value::Number(state.heading_deg as f64),
            Value::Number(state.heading_deg as f64),
            Value::Bool(true),
        ],
    );
    let location_changed = match last.as_ref() {
        None => true,
        Some(prev) => prev.location != state.location,
    };
    if location_changed {
        harness.call(
            "SetLocationName",
            vec![Value::String(state.location.clone())],
        );
    }
    let crosshair_changed = match last.as_ref() {
        None => true,
        Some(prev) => prev.crosshair != state.crosshair,
    };
    if crosshair_changed {
        match &state.crosshair {
            CrosshairTarget::None => {
                harness.call(
                    "SetCrosshairTarget",
                    vec![Value::Bool(false), Value::String(String::new())],
                );
            }
            CrosshairTarget::Target { name, weight, cost } => {
                let mut args = vec![
                    Value::Bool(true),
                    Value::String(name.clone()),
                    Value::Bool(true),
                    Value::Bool(false),
                    Value::Bool(false),
                    Value::Bool(true),
                ];
                // Weight/cost add the info lines (items only — a
                // defined zero would render a bogus "VALUE 0").
                if let (Some(weight), Some(cost)) = (weight, cost) {
                    args.push(Value::Number(*weight as f64));
                    args.push(Value::Number(*cost as f64));
                }
                harness.call("SetCrosshairTarget", args);
            }
        }
    }
    let hint_changed = match last.as_ref() {
        None => state.hint.is_some(),
        Some(prev) => prev.hint != state.hint,
    };
    if hint_changed {
        match &state.hint {
            Some(hint) => {
                harness.call(
                    "ShowTutorialHintText",
                    vec![Value::String(hint.clone()), Value::Bool(true)],
                );
            }
            None => {
                harness.call(
                    "ShowTutorialHintText",
                    vec![Value::String(String::new()), Value::Bool(false)],
                );
            }
        }
    }
    *last = Some(state.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::SpawnPoint;
    use crate::dialogue::TalkableNpc;
    use crate::doors::{DoorAnim, DoorRuntime};
    use crate::interact::{Container, Pickupable};
    use bevy::math::Quat;

    fn test_cell(interior: bool, form_id: u32, edid: &str) -> CellData {
        CellData {
            edid: edid.to_string(),
            form_id,
            interior,
            objects: Vec::new(),
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

    #[test]
    fn exterior_location_is_riverwood() {
        let cell = test_cell(false, 0, "Riverwood");
        assert_eq!(cell_location_name(&cell, None, None), "Riverwood");
    }

    #[test]
    fn interior_location_falls_back_to_prettified_edid() {
        let cell = test_cell(true, 0x000133c6, "RiverwoodSleepingGiantInn");
        assert_eq!(
            cell_location_name(&cell, None, None),
            "Riverwood Sleeping Giant Inn"
        );
    }

    #[test]
    fn hud_size_prefers_target_then_window_then_default() {
        assert_eq!(
            hud_size_for(Some((1920, 1080)), Some((800, 600))),
            (1280, 720)
        );
        assert_eq!(hud_size_for(None, Some((800, 600))), (800, 600));
        assert_eq!(hud_size_for(None, None), (1280, 720));
        assert_eq!(hud_size_for(Some((0, 0)), None), (1, 1));
    }

    /// Owned fixture for the crosshair tests: the resolver borrows
    /// from these, so they must outlive the `CrosshairInputs`
    /// (temporaries like `&Doors::default()` drop too early).
    #[derive(Default)]
    struct CrosshairFixture {
        doors: Doors,
        talkable: TalkableNpcs,
        pickupables: Pickupables,
        containers: Containers,
        objects: Vec<PlacedObject>,
        door_targets: Vec<DoorTrigger>,
        names: HashMap<u32, String>,
        defs: HashMap<u32, ItemDef>,
    }

    impl CrosshairFixture {
        fn inputs(&self) -> CrosshairInputs<'_> {
            CrosshairInputs {
                doors: &self.doors,
                talkable: &self.talkable,
                pickupables: &self.pickupables,
                containers: &self.containers,
                cell_objects: &self.objects,
                door_targets: &self.door_targets,
                plugin: None,
                strings: None,
                names: &self.names,
                defs: &self.defs,
            }
        }
    }

    /// A door 1 m in front of the feet (in USE range) with a
    /// generic "Wooden Door" base record — the inn door's shape.
    fn door_fixture(has_xtel: bool, target: Option<SceneTarget>) -> CrosshairFixture {
        let mut fixture = CrosshairFixture::default();
        fixture.doors.doors.insert(
            0x400,
            DoorRuntime::new(
                0x400,
                &Transform::from_translation(Vec3::new(0.0, 0.0, 1.0)),
                1.4,
                has_xtel,
                DoorAnim::fallback(),
                false,
            ),
        );
        fixture.objects.push(PlacedObject {
            ref_form_id: 0x400,
            base_form_id: 0x1,
            base_type: "DOOR".to_string(),
            base_edid: "WoodenDoor01".to_string(),
            model_path: None,
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            scale: 1.0,
            bounds: None,
            light: None,
            is_actor: false,
            is_creature: false,
            actor_models: Vec::new(),
            xtel: None,
        });
        if let Some(target) = target {
            fixture.door_targets.push(DoorTrigger {
                ref_form_id: 0x400,
                trigger_pos: [0.0, 0.0, 0.0],
                target,
                spawn: SpawnPoint {
                    position: [0.0, 0.0, 0.0],
                    yaw: 0.0,
                },
            });
        }
        fixture
    }

    #[test]
    fn crosshair_empty_scene_is_none() {
        let fixture = CrosshairFixture::default();
        assert_eq!(
            resolve_crosshair(&fixture.inputs(), Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0)),
            CrosshairTarget::None
        );
    }

    #[test]
    fn crosshair_npc_uses_index_name_then_edid() {
        let mut fixture = CrosshairFixture::default();
        fixture.talkable.npcs.insert(
            0x100,
            TalkableNpc {
                base_form_id: 0x13478,
                position: Vec3::new(0.0, 0.0, 1.0),
                base_edid: "Delphine".to_string(),
            },
        );
        fixture.names.insert(0x13478, "Delphine".to_string());
        assert_eq!(
            resolve_crosshair(&fixture.inputs(), Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0)),
            CrosshairTarget::Target {
                name: "Talk to Delphine".to_string(),
                weight: None,
                cost: None,
            }
        );
        // Without the index name, the prettified EDID stands in.
        fixture.names.clear();
        assert_eq!(
            resolve_crosshair(&fixture.inputs(), Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0)),
            CrosshairTarget::Target {
                name: "Talk to Delphine".to_string(),
                weight: None,
                cost: None,
            }
        );
    }

    #[test]
    fn crosshair_pickup_carries_weight_and_cost() {
        let mut fixture = CrosshairFixture::default();
        fixture.pickupables.items.insert(
            0x200,
            Pickupable {
                base_form_id: 0x1397e,
                position: Vec3::new(0.0, 0.0, 1.0),
                rotation: Quat::IDENTITY,
                harvest_name: None,
            },
        );
        fixture.defs.insert(
            0x1397e,
            ItemDef {
                base_form_id: 0x1397e,
                name: "Iron Dagger".to_string(),
                kind: crate::inventory::ItemKind::Weapon,
                value: 10,
                weight: 2.5,
                damage: 4,
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
        assert_eq!(
            resolve_crosshair(&fixture.inputs(), Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0)),
            CrosshairTarget::Target {
                name: "Take Iron Dagger".to_string(),
                weight: Some(2.5),
                cost: Some(10),
            }
        );
    }

    #[test]
    fn crosshair_harvest_uses_plant_name_without_lines() {
        let mut fixture = CrosshairFixture::default();
        fixture.pickupables.items.insert(
            0x200,
            Pickupable {
                base_form_id: 0x1,
                position: Vec3::new(0.0, 0.0, 1.0),
                rotation: Quat::IDENTITY,
                harvest_name: Some("Mountain Flower".to_string()),
            },
        );
        assert_eq!(
            resolve_crosshair(&fixture.inputs(), Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0)),
            CrosshairTarget::Target {
                name: "Take Mountain Flower".to_string(),
                weight: None,
                cost: None,
            }
        );
    }

    #[test]
    fn crosshair_load_door_shows_destination_not_base_name() {
        // The inn door's base record is a generic "Wooden Door";
        // the real game prompts the destination. No plugin/strings
        // in the fixture, so the destination falls back to the
        // prettified destination EDID.
        let fixture = door_fixture(
            true,
            Some(SceneTarget::Interior {
                form_id: 0,
                edid: "RiverwoodSleepingGiantInn".to_string(),
            }),
        );
        assert_eq!(
            resolve_crosshair(&fixture.inputs(), Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0)),
            CrosshairTarget::Target {
                name: "Open Riverwood Sleeping Giant Inn".to_string(),
                weight: None,
                cost: None,
            }
        );
    }

    #[test]
    fn crosshair_exterior_load_door_shows_riverwood() {
        let fixture = door_fixture(true, Some(SceneTarget::Exterior { grid: (5, -11) }));
        assert_eq!(
            resolve_crosshair(&fixture.inputs(), Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0)),
            CrosshairTarget::Target {
                name: "Open Riverwood".to_string(),
                weight: None,
                cost: None,
            }
        );
    }

    #[test]
    fn crosshair_room_door_shows_door_record_name() {
        // No trigger (room door): the door record's FULL, which
        // is what the real game shows for room doors.
        let fixture = door_fixture(false, None);
        assert_eq!(
            resolve_crosshair(&fixture.inputs(), Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0)),
            CrosshairTarget::Target {
                name: "Open Wooden Door 01".to_string(),
                weight: None,
                cost: None,
            }
        );
    }

    #[test]
    fn crosshair_container_uses_prettified_edid() {
        let mut fixture = CrosshairFixture::default();
        fixture.containers.items.insert(
            0x300,
            Container {
                position: Vec3::new(0.0, 0.0, 1.0),
                contents: Vec::new(),
            },
        );
        fixture.objects.push(PlacedObject {
            ref_form_id: 0x300,
            base_form_id: 0x1,
            base_type: "CONT".to_string(),
            base_edid: "RiverwoodSleepingGiantInnChest".to_string(),
            model_path: None,
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            scale: 1.0,
            bounds: None,
            light: None,
            is_actor: false,
            is_creature: false,
            actor_models: Vec::new(),
            xtel: None,
        });
        assert_eq!(
            resolve_crosshair(&fixture.inputs(), Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0)),
            CrosshairTarget::Target {
                name: "Search Riverwood Sleeping Giant Inn Chest".to_string(),
                weight: None,
                cost: None,
            }
        );
    }

    #[test]
    fn crosshair_ignores_unfaced_targets() {
        // v0.4.13: the crosshair named Embry while Dylan aimed at
        // the fire pit. An NPC in range but behind the player is
        // no target; facing her names her (with the verb).
        let mut fixture = CrosshairFixture::default();
        fixture.talkable.npcs.insert(
            0x100,
            TalkableNpc {
                base_form_id: 0x13478,
                position: Vec3::new(0.0, 0.0, 1.0),
                base_edid: "Delphine".to_string(),
            },
        );
        fixture.names.insert(0x13478, "Delphine".to_string());
        // Looking away (-Z; the NPC is at +Z): nothing.
        assert_eq!(
            resolve_crosshair(
                &fixture.inputs(),
                Vec3::ZERO,
                Vec3::new(0.0, 0.0, -1.0)
            ),
            CrosshairTarget::None
        );
        // Looking at her (+Z): the verb + name.
        assert_eq!(
            resolve_crosshair(
                &fixture.inputs(),
                Vec3::ZERO,
                Vec3::new(0.0, 0.0, 1.0)
            ),
            CrosshairTarget::Target {
                name: "Talk to Delphine".to_string(),
                weight: None,
                cost: None,
            }
        );
    }

    /// v0.4.9 diagnosis: does a panic inside a system propagate
    /// out of `App::update`? The code says yes (the multi-threaded
    /// executor catches the panic, the default error handler
    /// re-panics via `resume_unwind`, the executor resumes it at
    /// schedule end), but the v0.4.9 phone log shows `app.run()`
    /// RETURNING after a render-system panic — so this test pins
    /// the actual behaviour. If the panic is swallowed here, the
    /// phone's clean exit is explained by the executor, and the
    /// exit came via the RenderError/AppExit path (see
    /// docs/v049-crash-diagnosis.md).
    #[test]
    fn system_panic_propagates_out_of_app_update() {
        fn panicking_system() {
            panic!("intentional test panic");
        }
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .add_systems(Update, panicking_system);
        // The panic hook prints the panic to stderr — expected
        // noise for this test (it proves the panic fired).
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.update();
        }));
        assert!(
            result.is_err(),
            "a system panic must propagate out of App::update (the executor resumes it)"
        );
    }

    /// v0.4.11 perf work: the Ruffle cost window aggregates step
    /// + capture costs and flushes once per 5 s window.
    #[test]
    fn ruffle_cost_stats_flush_reports_averages_and_maxima() {
        let mut stats = RuffleCostStats::default();
        // No steps yet: nothing to flush, whenever asked.
        assert!(stats
            .flush(Instant::now() + std::time::Duration::from_secs(6))
            .is_none());
        let step = |run: f64, render: f64| FrameTiming {
            frame: 0,
            preload_run_ms: run,
            render_ms: render,
        };
        stats.record_step(&step(10.0, 20.0));
        stats.record_step(&step(12.0, 90.0));
        stats.record_capture(5.0);
        stats.record_capture(25.0);
        // Window not due yet (started at the first step).
        assert!(stats.flush(Instant::now()).is_none());
        let line = stats
            .flush(Instant::now() + std::time::Duration::from_secs(6))
            .expect("due window flushes");
        assert!(line.contains("window 0: 2 steps"), "line was: {line}");
        assert!(
            line.contains("run avg 11.0ms max 12.0ms"),
            "line was: {line}"
        );
        assert!(
            line.contains("render avg 55.0ms max 90.0ms"),
            "line was: {line}"
        );
        assert!(
            line.contains("2 captures, capture avg 15.0ms max 25.0ms"),
            "line was: {line}"
        );
        // The next window starts clean at index 1.
        stats.record_step(&step(1.0, 2.0));
        let line2 = stats
            .flush(Instant::now() + std::time::Duration::from_secs(12))
            .expect("second window flushes");
        assert!(line2.contains("window 1: 1 steps"), "line was: {line2}");
        assert!(
            line2.contains("0 captures, capture avg 0.0ms max 0.0ms"),
            "line was: {line2}"
        );
    }

    /// v0.4.11 perf work: the movie step gate never steps more
    /// than once per render frame, and drops any backlog beyond
    /// one movie frame.
    #[test]
    fn movie_steps_due_caps_at_one_step_and_clamps_the_backlog() {
        // 60 fps: no step on the first frame, one on the second
        // (two 16.7 ms deltas cross the 33.3 ms movie frame).
        let (steps, accum) = movie_steps_due(0.0, 1.0 / 60.0, false);
        assert_eq!(steps, 0);
        assert!((accum - 1.0 / 60.0).abs() < 1e-6);
        let (steps, accum) = movie_steps_due(accum, 1.0 / 60.0, false);
        assert_eq!(steps, 1);
        assert!(accum < MOVIE_FRAME_SECS);
        // Below 30 fps: exactly one step, and the leftover
        // backlog is clamped to one movie frame (not 66.7 ms).
        let (steps, accum) = movie_steps_due(0.0, 0.1, false);
        assert_eq!(steps, 1);
        assert!((accum - MOVIE_FRAME_SECS).abs() < 1e-6);
        // A huge backlog (a 2.8 s frame on a slow device): still
        // one step, still clamped to one movie frame.
        let (steps, accum) = movie_steps_due(2.8, 0.1, false);
        assert_eq!(steps, 1);
        assert!((accum - MOVIE_FRAME_SECS).abs() < 1e-6);
        // Forced first step: one step even with no clock debt,
        // and no clock consumption.
        let (steps, accum) = movie_steps_due(0.0, 0.0, true);
        assert_eq!(steps, 1);
        assert_eq!(accum, 0.0);
        // Forced first step with the clock also due: still one
        // step, and the clock frame is consumed.
        let (steps, accum) = movie_steps_due(0.04, 0.0, true);
        assert_eq!(steps, 1);
        assert!((accum - (0.04 - MOVIE_FRAME_SECS)).abs() < 1e-6);
    }
}
