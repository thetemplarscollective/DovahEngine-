//! The harness runner (M1): build the Ruffle player on its own
//! offscreen wgpu device, drive frames with stub game data, capture
//! screenshots.
//!
//! M4 adds the engine-facing constructors: [`Harness::new_from_bytes`]
//! (the SWF/fontlib come from the BSA, not disk) and
//! [`Harness::new_shared`] (the player runs on the host's wgpu device
//! — Bevy's — instead of its own). The binary keeps using
//! [`Harness::new`]; the engine uses `new_shared`. All three share
//! the private [`Harness::from_parts`], so there is one bridge, not
//! two.
//!
//! Shape (mirrors Ruffle's own `exporter` package, the closest
//! reference implementation):
//!
//! * [`WgpuRenderBackend::<TextureTarget>::for_offscreen`] creates
//!   the renderer on its OWN wgpu device — no Bevy coupling in M1.
//! * [`PlayerBuilder`] wires the stub [`crate::navigator`] (fontlib
//!   from memory) and stub [`crate::provider`] (log + canned
//!   answers), loads the movie with
//!   [`ruffle_core::tag_utils::movie_from_path`], and builds an
//!   `Arc<Mutex<Player>>`.
//! * Each frame: pump the [`NullExecutor`], `preload` + `run_frame`,
//!   pump again, `render`. The executor pump is what completes the
//!   fontlib import — without it the loader's futures never run.
//! * Capture goes through the exporter's downcast:
//!   `player.renderer_mut()` → `WgpuRenderBackend<TextureTarget>` →
//!   `capture_frame()`.
//!
//! Game → SWF calls go through
//! [`Player::call_internal_interface`] with the `"call"` callback:
//! `call("SetHealthMeterPercent", [0.75, false])` reaches
//! `GameDelegate.receiveCall` → the HUD's registered handler. The
//! HUD's handler signatures are in the feasibility study §2.2.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use image::RgbaImage;
use ruffle_core::backend::locale::DeterministicLocaleBackend;
use ruffle_core::backend::navigator::NullExecutor;
use ruffle_core::external::Value;
use ruffle_core::limits::ExecutionLimit;
use ruffle_core::tag_utils::{movie_from_path, SwfMovie};
use ruffle_core::{Player, PlayerBuilder};
use ruffle_render_wgpu::backend::WgpuRenderBackend;
use ruffle_render_wgpu::descriptors::Descriptors;
use ruffle_render_wgpu::target::TextureTarget;
use ruffle_render_wgpu::wgpu;

use crate::navigator::MemoryNavigatorBackend;
use crate::provider::{ProviderState, StubProvider};

/// What the harness renders at: the study's desktop runs used
/// 1280×720 @ 30 fps, and the reference screenshot
/// (`shot-hud-fontlib.png`) is that size.
pub const DEFAULT_WIDTH: u32 = 1280;
pub const DEFAULT_HEIGHT: u32 = 720;
pub const DEFAULT_FRAME_RATE: f64 = 30.0;

/// Frames to step before driving stub data, so the movie's frame-1
/// scripts run and the fontlib import completes.
pub const SETTLE_FRAMES: u32 = 10;
/// Frames to step after driving, so the meter fade-in anims play.
/// Frames stepped after the drive before capturing: the meter fade-in
/// (`RunMeterAnim` → `PlayForward`) and the location-name fade-in
/// (`SetLocationName` → `gotoAndPlay(1)`) both start at alpha 0 and need
/// ~1s to become visible — 15 frames caught them still transparent.
pub const DRIVE_FRAMES: u32 = 45;

/// Errors the harness can report. All variants carry a message; the
/// binary prints them and exits nonzero.
#[derive(Debug)]
pub enum HarnessError {
    /// Reading the SWF or fontlib file failed.
    Io(std::io::Error),
    /// The SWF did not parse.
    Movie(String),
    /// The wgpu renderer (device, adapter, target) failed to come up.
    Render(String),
    /// `capture_frame` returned nothing — no frame was submitted.
    NoFrame,
    /// The fontlib file exists but is empty.
    EmptyFontlib,
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HarnessError::Io(e) => write!(f, "I/O error: {e}"),
            HarnessError::Movie(e) => write!(f, "SWF load error: {e}"),
            HarnessError::Render(e) => write!(f, "renderer error: {e}"),
            HarnessError::NoFrame => write!(f, "no frame captured"),
            HarnessError::EmptyFontlib => write!(f, "fontlib file is empty"),
        }
    }
}

impl std::error::Error for HarnessError {}

/// What to run.
#[derive(Debug, Clone)]
pub struct HarnessConfig {
    /// The menu SWF (M1: `interface-extract/study/hudmenu.gfx`).
    pub swf_path: PathBuf,
    /// The synthesized fontlib
    /// (`interface-extract/tools/ruffle-test/gfxfontlib.swf`).
    pub fontlib_path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
}

impl HarnessConfig {
    pub fn new(swf_path: PathBuf, fontlib_path: PathBuf) -> Self {
        Self {
            swf_path,
            fontlib_path,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            frame_rate: DEFAULT_FRAME_RATE,
        }
    }
}

/// Per-frame cost, split the way it matters: `preload_run_ms` is the
/// AVM1 + loader work (`preload` + `run_frame`), `render_ms` is the
/// command-list build + submit (`render`).
#[derive(Debug, Clone, Copy)]
pub struct FrameTiming {
    pub frame: u32,
    pub preload_run_ms: f64,
    pub render_ms: f64,
}

/// Mean / p95 / max over a run. The first frames include movie load
/// and fontlib import, so callers that want steady-state cost should
/// summarize the post-settle slice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimingSummary {
    pub frames: usize,
    pub run_mean_ms: f64,
    pub run_p95_ms: f64,
    pub run_max_ms: f64,
    pub render_mean_ms: f64,
    pub render_max_ms: f64,
}

pub fn summarize(timings: &[FrameTiming]) -> TimingSummary {
    let frames = timings.len();
    if frames == 0 {
        return TimingSummary {
            frames: 0,
            run_mean_ms: 0.0,
            run_p95_ms: 0.0,
            run_max_ms: 0.0,
            render_mean_ms: 0.0,
            render_max_ms: 0.0,
        };
    }
    let mut runs: Vec<f64> = timings.iter().map(|t| t.preload_run_ms).collect();
    runs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let run_mean_ms = runs.iter().sum::<f64>() / frames as f64;
    let run_p95_ms = runs[(frames * 95 / 100).min(frames - 1)];
    let run_max_ms = runs[frames - 1];
    let render_mean_ms = timings.iter().map(|t| t.render_ms).sum::<f64>() / frames as f64;
    let render_max_ms = timings.iter().map(|t| t.render_ms).fold(0.0f64, f64::max);
    TimingSummary {
        frames,
        run_mean_ms,
        run_p95_ms,
        run_max_ms,
        render_mean_ms,
        render_max_ms,
    }
}

/// The host's wgpu handles, for sharing one device with the engine
/// (M4). Bevy 0.20 and Ruffle both use wgpu 30 — Cargo unifies them
/// to a single version (30.0.2 in the lockfile), so the handles the
/// engine clones out of its render world are the same types Ruffle
/// expects. `Descriptors::new` takes the device by value AND the
/// `TextureTarget` needs a reference to it, so the device is cloned
/// once inside [`Harness::new_shared`].
pub struct SharedWgpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// The M1 harness: one Ruffle player, offscreen, driven by stub data.
pub struct Harness {
    player: Arc<Mutex<Player>>,
    executor: NullExecutor,
    provider_state: Arc<Mutex<ProviderState>>,
    frames_stepped: u32,
}

impl Harness {
    /// Build the player. Reads the fontlib bytes and the SWF, creates
    /// the offscreen wgpu device, and wires the stub backends. This
    /// is the slow step (wgpu device init + shader compile) — the
    /// binary times it separately.
    pub fn new(config: &HarnessConfig) -> Result<Self, HarnessError> {
        let fontlib = fs::read(&config.fontlib_path).map_err(HarnessError::Io)?;
        if fontlib.is_empty() {
            return Err(HarnessError::EmptyFontlib);
        }
        let movie = movie_from_path(&config.swf_path, None)
            .map_err(|e| HarnessError::Movie(e.to_string()))?;
        let base_path = config
            .swf_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let renderer: WgpuRenderBackend<TextureTarget> = WgpuRenderBackend::for_offscreen(
            (config.width, config.height),
            wgpu::Backends::VULKAN,
            wgpu::PowerPreference::HighPerformance,
        )
        .map_err(|e| HarnessError::Render(e.to_string()))?;
        Self::from_parts(
            movie,
            fontlib,
            base_path,
            config.width,
            config.height,
            config.frame_rate,
            renderer,
        )
    }

    /// Build from in-memory bytes (M4): the engine reads the SWF and
    /// the fontlib from the BSA, so there are no files on disk. The
    /// movie's base URL is built from `base_path` + `movie_name`
    /// (relative imports resolve against the data dir); the file
    /// itself need not exist on disk. Still creates its OWN offscreen
    /// device — the engine path is [`Harness::new_shared`].
    pub fn new_from_bytes(
        swf_bytes: &[u8],
        fontlib_bytes: &[u8],
        movie_name: &str,
        base_path: PathBuf,
        width: u32,
        height: u32,
        frame_rate: f64,
    ) -> Result<Self, HarnessError> {
        if fontlib_bytes.is_empty() {
            return Err(HarnessError::EmptyFontlib);
        }
        let url = movie_url(&base_path, movie_name);
        let movie = SwfMovie::from_data(swf_bytes, url, None, None)
            .map_err(|e| HarnessError::Movie(e.to_string()))?;
        let renderer: WgpuRenderBackend<TextureTarget> = WgpuRenderBackend::for_offscreen(
            (width, height),
            wgpu::Backends::VULKAN,
            wgpu::PowerPreference::HighPerformance,
        )
        .map_err(|e| HarnessError::Render(e.to_string()))?;
        Self::from_parts(
            movie,
            fontlib_bytes.to_vec(),
            base_path,
            width,
            height,
            frame_rate,
            renderer,
        )
    }

    /// Build on the host's wgpu device (M4): the engine shares
    /// Bevy's device instead of creating a second one — one device,
    /// one queue, no second shader compile. The render target is a
    /// `TextureTarget` at the HUD size; the engine reads the pixels
    /// back with [`Harness::capture`] and composites them as a UI
    /// image.
    // Eight parameters, each a distinct input (device handles,
    // movie bytes, fontlib bytes, name, base path, size, rate) —
    // bundling them into a config struct would only move the
    // same list elsewhere. The render system allows the same
    // lint for the same reason.
    #[allow(clippy::too_many_arguments)]
    pub fn new_shared(
        shared: SharedWgpu,
        swf_bytes: &[u8],
        fontlib_bytes: &[u8],
        movie_name: &str,
        base_path: PathBuf,
        width: u32,
        height: u32,
        frame_rate: f64,
    ) -> Result<Self, HarnessError> {
        if fontlib_bytes.is_empty() {
            return Err(HarnessError::EmptyFontlib);
        }
        let url = movie_url(&base_path, movie_name);
        let movie = SwfMovie::from_data(swf_bytes, url, None, None)
            .map_err(|e| HarnessError::Movie(e.to_string()))?;
        let descriptors = Arc::new(Descriptors::new(
            shared.instance,
            shared.adapter,
            shared.device.clone(),
            shared.queue,
        ));
        let target = TextureTarget::new(&shared.device, (width, height))
            .map_err(|e| HarnessError::Render(e.to_string()))?;
        let renderer = WgpuRenderBackend::new(descriptors, target)
            .map_err(|e| HarnessError::Render(e.to_string()))?;
        Self::from_parts(
            movie,
            fontlib_bytes.to_vec(),
            base_path,
            width,
            height,
            frame_rate,
            renderer,
        )
    }

    /// The shared core: executor, navigator (fontlib from memory),
    /// provider (stub GameDelegate), and the player itself. Every
    /// constructor funnels through here, so the bridge is one copy.
    fn from_parts(
        movie: SwfMovie,
        fontlib: Vec<u8>,
        base_path: PathBuf,
        width: u32,
        height: u32,
        frame_rate: f64,
        renderer: WgpuRenderBackend<TextureTarget>,
    ) -> Result<Self, HarnessError> {
        let executor = NullExecutor::new();
        let navigator = MemoryNavigatorBackend::new(fontlib, base_path, &executor);
        let provider = StubProvider::new();
        let provider_state = provider.state();
        let player = PlayerBuilder::new()
            .with_renderer(renderer)
            .with_locale(DeterministicLocaleBackend::default())
            .with_navigator(navigator)
            .with_external_interface(Box::new(provider))
            .with_movie(movie)
            .with_viewport_dimensions(width, height, 1.0)
            .with_frame_rate(Some(frame_rate))
            .build();
        Ok(Self {
            player,
            executor,
            provider_state,
            frames_stepped: 0,
        })
    }

    /// HUD compositing (M4): render with a transparent background so
    /// the 3D scene shows through the HUD texture. The harness
    /// binary does NOT call this (its captures keep the white
    /// background the study's screenshots show).
    pub fn set_transparent(&mut self) {
        self.player
            .lock()
            .expect("player lock")
            .set_window_mode("transparent");
    }

    /// Safe-area insets (M4, patch 0007): pixels of notch/gesture-bar
    /// inset on each edge, narrowing `Stage.safeRect` (the HUD
    /// anchors its elements to it). Zero on desktop.
    pub fn set_safe_area_insets(&mut self, left: u32, top: u32, right: u32, bottom: u32) {
        self.player
            .lock()
            .expect("player lock")
            .set_safe_area_insets(left, top, right, bottom);
    }

    /// Readable handle to the provider's call log.
    pub fn provider_state(&self) -> Arc<Mutex<ProviderState>> {
        Arc::clone(&self.provider_state)
    }

    pub fn frames_stepped(&self) -> u32 {
        self.frames_stepped
    }

    /// One full frame: pump the executor, `preload` + `run_frame`,
    /// pump again, `render`. Returns the frame's cost split.
    pub fn step(&mut self) -> FrameTiming {
        self.executor.run();
        let started = Instant::now();
        {
            let mut player = self.player.lock().expect("player lock");
            player.preload(&mut ExecutionLimit::none());
            player.run_frame();
        }
        let preload_run_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.executor.run();
        let started = Instant::now();
        self.player.lock().expect("player lock").render();
        let render_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.frames_stepped += 1;
        FrameTiming {
            frame: self.frames_stepped,
            preload_run_ms,
            render_ms,
        }
    }

    /// Logic-only frame: pump the executor, `preload` + `run_frame`,
    /// pump again — NO `render`. The settle phase (v0.4.9) uses
    /// this: settle frames are never captured, so rasterizing them
    /// is wasted GPU work, and on the phone each full step may cost
    /// ~800 ms (v0.4.9 diagnosis — the HUD never reached its first
    /// capture in 8.4 s). The fontlib import completes via the
    /// executor pump + preload, and frame-1 scripts run in
    /// `run_frame` — neither needs a render. `preload_run_ms`
    /// covers the whole logic phase (both executor pumps included);
    /// `render_ms` is 0.
    pub fn step_logic(&mut self) -> FrameTiming {
        let started = Instant::now();
        self.executor.run();
        {
            let mut player = self.player.lock().expect("player lock");
            player.preload(&mut ExecutionLimit::none());
            player.run_frame();
        }
        self.executor.run();
        let preload_run_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.frames_stepped += 1;
        FrameTiming {
            frame: self.frames_stepped,
            preload_run_ms,
            render_ms: 0.0,
        }
    }

    /// Game → SWF: invoke a `GameDelegate`-registered handler.
    /// `call("SetHealthMeterPercent", [0.75, false])` reaches the
    /// HUD's `SetHealthMeterPercent(aPercent, abForce)`.
    ///
    /// Only works AFTER the movie's `InitExtensions` has run (see
    /// [`Self::call_root_function`]) — that is what registers the
    /// GameDelegate `"call"` callback in the first place.
    pub fn call(&mut self, method: &str, args: Vec<Value>) -> Value {
        let mut full: Vec<Value> = Vec::with_capacity(args.len() + 1);
        full.push(Value::String(method.to_string()));
        full.extend(args);
        self.player
            .lock()
            .expect("player lock")
            .call_internal_interface("call", full)
    }

    /// Game → SWF: invoke a function defined on the root timeline,
    /// e.g. the frame-1 global `InitExtensions`. Uses the Dovahengine
    /// `Player::call_root_function` patch (vendored Ruffle) — stock
    /// Ruffle 0.7.1 has no public API for this.
    pub fn call_root_function(&mut self, name: &str, args: Vec<Value>) -> Value {
        self.player
            .lock()
            .expect("player lock")
            .call_root_function(name, args)
    }

    /// Game → SWF: invoke a method on a nested clip, resolved by
    /// dotted path from the root (e.g. `"Health"`,
    /// `"LocationLockBase.LocationNameBase"`). Uses the Dovahengine
    /// `Player::call_clip_function` patch — the equivalent of
    /// Scaleform's `Invoke` on a named clip.
    pub fn call_clip_function(&mut self, path: &str, name: &str, args: Vec<Value>) -> Value {
        self.player
            .lock()
            .expect("player lock")
            .call_clip_function(path, name, args)
    }

    /// Game → SWF: set a variable on a nested clip, resolved by
    /// dotted path from the root (e.g. `("InventoryLists_mc.ItemsList",
    /// "entryList", [...])`). Uses the Dovahengine
    /// `Player::set_clip_variable` patch — Scaleform's `SetVariable`,
    /// the only way to populate a menu's `entryList` (no GameDelegate
    /// handler exists for it). Returns `false` if the path does not
    /// resolve.
    pub fn set_clip_variable(&mut self, path: &str, name: &str, value: Value) -> bool {
        self.player
            .lock()
            .expect("player lock")
            .set_clip_variable(path, name, value)
    }

    /// Capture the last rendered frame as an RGBA image, or `None`
    /// if no frame was submitted yet.
    pub fn capture(&mut self) -> Option<RgbaImage> {
        let mut player = self.player.lock().expect("player lock");
        let renderer =
            <dyn Any>::downcast_mut::<WgpuRenderBackend<TextureTarget>>(player.renderer_mut())?;
        renderer.capture_frame()
    }

    /// The M1 stub-data drive: what the game would report for a
    /// healthy player standing in Riverwood looking at Delphine.
    ///
    /// `InitExtensions` runs FIRST — it registers every GameDelegate
    /// callback, so the `call(...)` drive below only works after it.
    /// Meter percents use `abForce = false` with changed values, so
    /// the SWF runs its meter fade-in anim — the vanilla auto-hide
    /// behaviour lives in the SWF (feasibility §2.2). The crosshair
    /// passes only 6 args (no weight/cost — an NPC has none, and
    /// defined zeroes would render a bogus info line). The tutorial
    /// hint carries a `[Activate]` token, which makes the SWF call
    /// `GetButtonFromUserEvent` — the SWF→game round trip.
    ///
    /// Percents are 0–100, NOT 0–1 fractions: `Components.Meter`
    /// clamps to 0–100 and lerps the bar clip's Empty→Full frames
    /// over 0–100 (`Meter.as`). M1 passed 0.75/0.6/0.9, which the
    /// SWF read as ~0% — the bars drained to their Empty frames
    /// (M2 root cause for the missing meters).
    pub fn drive_hud_demo(&mut self) {
        self.call_root_function("InitExtensions", vec![]);
        self.call("SetHealthMeterPercent", vec![75.0.into(), false.into()]);
        self.call("SetMagickaMeterPercent", vec![60.0.into(), false.into()]);
        self.call("SetStaminaMeterPercent", vec![90.0.into(), false.into()]);
        self.call("SetCompassAngle", vec![0.0.into(), 0.0.into(), true.into()]);
        self.call("SetLocationName", vec!["Riverwood".into()]);
        self.call(
            "SetCrosshairTarget",
            vec![
                true.into(),
                "Delphine".into(),
                true.into(),
                false.into(),
                false.into(),
                true.into(),
            ],
        );
        self.call(
            "ShowTutorialHintText",
            vec!["[Activate] Talk to Delphine".into(), true.into()],
        );
    }

    /// Diagnostic probe (M1): the normal demo drive, then jump the
    /// meter/location clips straight to mid-timeline frames. The
    /// meter clips are 200 frames and the location clip 170; frame
    /// 100 / 85 should be mid-hold at full alpha. If the meters and
    /// "Riverwood" appear after the jump, the timelines render fine
    /// and the fault is in playing advancement (`gotoAndPlay`); if
    /// they still do not appear, the content itself never renders.
    /// Paths are from the ROOT clip — the HUD elements live on the
    /// `HUDMovieBaseInstance` child, not the root itself.
    pub fn drive_hud_probe(&mut self) {
        self.drive_hud_demo();
        self.jump_meters_location();
    }

    /// M2 diagnostic: `InitExtensions` + ONLY `SetLocationName`.
    /// Isolates the location name from the rest of the drive: if
    /// "Riverwood" appears here, something else in the full drive
    /// hides it; if not, `SetLocationName` itself (its `SetText` or
    /// its `gotoAndPlay(1)`) is the fault.
    pub fn drive_hud_loc(&mut self) {
        self.call_root_function("InitExtensions", vec![]);
        self.call("SetLocationName", vec!["Riverwood".into()]);
    }

    /// M2 diagnostic: `InitExtensions` + the probe jumps ONLY — no
    /// `SetLocationName`, no meter percents. If the DEFAULT
    /// "Location" text appears at frame 85, the probe path/method
    /// works and the location timeline content is fine, which pins
    /// the fault on `SetLocationName`'s `SetText`/`gotoAndPlay`.
    /// If nothing appears, the probe itself is suspect (or the
    /// timeline content never renders at any frame).
    pub fn drive_hud_jump(&mut self) {
        self.call_root_function("InitExtensions", vec![]);
        self.jump_meters_location();
    }

    /// M2 diagnostic: `InitExtensions` + FORCED percents (25/50/90).
    /// The forced path calls `SetPercent` directly, bypassing the
    /// `onEnterFrame` → `Update()` tween. If the bars land at their
    /// forced frames, the Meter + `gotoAndStop` machinery works and
    /// any stuck-at-Full fault is in `onEnterFrame`/`Update`.
    pub fn drive_hud_force(&mut self) {
        self.call_root_function("InitExtensions", vec![]);
        self.call("SetHealthMeterPercent", vec![25.0.into(), true.into()]);
        self.call("SetMagickaMeterPercent", vec![50.0.into(), true.into()]);
        self.call("SetStaminaMeterPercent", vec![90.0.into(), true.into()]);
        self.call("SetLocationName", vec!["Riverwood".into()]);
    }

    /// M2 diagnostic: `InitExtensions` + UNFORCED percents (25/50/90),
    /// then 10 DIRECT `Update()` calls on each Meter object (via
    /// `call_clip_function` — the Meters are plain AS objects on
    /// `HUDMovieBaseInstance`, not clips). If the bars move, `Update`
    /// works and the fault is `onEnterFrame` not firing; if they
    /// stay at Full, `Update` itself is broken.
    pub fn drive_hud_upd(&mut self) {
        self.call_root_function("InitExtensions", vec![]);
        self.call("SetHealthMeterPercent", vec![25.0.into(), false.into()]);
        self.call("SetMagickaMeterPercent", vec![50.0.into(), false.into()]);
        self.call("SetStaminaMeterPercent", vec![90.0.into(), false.into()]);
        self.call("SetLocationName", vec!["Riverwood".into()]);
        for _ in 0..10 {
            for meter in [
                "HUDMovieBaseInstance.HealthMeterLeft",
                "HUDMovieBaseInstance.MagickaMeter",
                "HUDMovieBaseInstance.StaminaMeter",
            ] {
                self.call_clip_function(meter, "Update", vec![]);
            }
        }
    }

    /// The M3 inventory drive: populate the inventory menu with stub
    /// game data — real categories, five items, player info — and
    /// open the items panel, so all three components render:
    /// InventoryLists (categories + items), ItemCard (the selected
    /// Iron Dagger, via the `RequestItemCardInfo` respond round
    /// trip), BottomBar (player stats). Returns `true` if the
    /// `entryList` injection landed (the one step whose failure is
    /// silent — a wrong clip path would leave the items panel
    /// empty with no error).
    ///
    /// Category flags are one bit per category (Weapons 1, Apparel
    /// 2, ...); "All" carries every bit (511). Items carry the
    /// matching `filterFlag` bits. All categories pass
    /// `bDontHide = true` so every category shows.
    pub fn drive_inventory_demo(&mut self) -> bool {
        self.call_root_function("InitExtensions", vec![]);
        let mut categories: Vec<Value> = Vec::new();
        for (text, flag) in [
            ("All", 511.0),
            ("Weapons", 1.0),
            ("Apparel", 2.0),
            ("Potions", 4.0),
            ("Scrolls", 8.0),
            ("Food", 16.0),
            ("Ingredients", 32.0),
            ("Books", 64.0),
            ("Keys", 128.0),
            ("Misc", 256.0),
        ] {
            categories.push(text.into());
            categories.push(flag.into());
            categories.push(true.into());
        }
        self.call("SetCategoriesList", categories);
        let items = Value::List(vec![
            inventory_entry("Iron Dagger", 1.0, 1.0),
            inventory_entry("Iron Sword", 1.0, 1.0),
            inventory_entry("Leather Armor", 1.0, 2.0),
            inventory_entry("Minor Healing", 3.0, 4.0),
            inventory_entry("Bread", 2.0, 16.0),
        ]);
        let injected =
            self.set_clip_variable("Menu_mc.InventoryLists_mc.ItemsList", "entryList", items);
        self.call("InvalidateListData", vec![]);
        // Open the items panel (no sound, with the slide-in
        // animation). The showItemsList event fires synchronously:
        // the SWF requests the card info, the provider answers with
        // the canned Iron Dagger, and the card fades in.
        // NOTE: the components live under the `Menu_mc` clip the
        // root places (char 91, `InventoryMenuObj`) — not on the
        // root itself. The card sits one level deeper, inside
        // `ItemCardFadeHolder_mc`.
        self.call_clip_function(
            "Menu_mc.InventoryLists_mc",
            "ShowItemsList",
            vec![false.into(), true.into()],
        );
        let mut player = BTreeMap::new();
        player.insert("armor".to_string(), Value::Number(0.0));
        player.insert("damage".to_string(), Value::Number(4.0));
        player.insert("encumbrance".to_string(), Value::Number(45.0));
        player.insert("maxEncumbrance".to_string(), Value::Number(300.0));
        player.insert("gold".to_string(), Value::Number(127.0));
        player.insert("health".to_string(), Value::Number(100.0));
        player.insert("maxHealth".to_string(), Value::Number(100.0));
        player.insert("magicka".to_string(), Value::Number(100.0));
        player.insert("maxMagicka".to_string(), Value::Number(100.0));
        player.insert("stamina".to_string(), Value::Number(100.0));
        player.insert("maxStamina".to_string(), Value::Number(100.0));
        self.call("UpdatePlayerInfo", vec![Value::Object(player)]);
        injected
    }

    /// Jump the meter clips to frame 100 and the location clip to
    /// frame 85 (mid-timeline, mid-hold at full alpha — if the
    /// timelines render at all). Shared by the probe and jump modes.
    fn jump_meters_location(&mut self) {
        self.call_clip_function(
            "HUDMovieBaseInstance.Health",
            "gotoAndStop",
            vec![100.0.into()],
        );
        self.call_clip_function(
            "HUDMovieBaseInstance.Magica",
            "gotoAndStop",
            vec![100.0.into()],
        );
        self.call_clip_function(
            "HUDMovieBaseInstance.Stamina",
            "gotoAndStop",
            vec![100.0.into()],
        );
        self.call_clip_function(
            "HUDMovieBaseInstance.LocationLockBase.LocationNameBase",
            "gotoAndStop",
            vec![85.0.into()],
        );
    }

    /// The clip paths the probe jumps, from the root clip. Used by
    /// the binary to verify path resolution before trusting a probe
    /// result: `call_clip_function` returns Undefined on a missing
    /// path AND `gotoAndStop` returns Undefined on success, so a
    /// jump's return value cannot tell them apart — but `getDepth`
    /// returns the clip's depth number on success.
    pub const PROBE_CLIP_PATHS: [&'static str; 4] = [
        "HUDMovieBaseInstance.Health",
        "HUDMovieBaseInstance.Magica",
        "HUDMovieBaseInstance.Stamina",
        "HUDMovieBaseInstance.LocationLockBase.LocationNameBase",
    ];

    /// `MovieClip.getDepth()` on a nested clip, resolved by dotted
    /// path from the root. Returns the depth number on success, or
    /// Undefined if the path does not resolve.
    pub fn clip_depth(&mut self, path: &str) -> Value {
        self.call_clip_function(path, "getDepth", vec![])
    }

    /// Clips whose runtime state the binary dumps after the drive:
    /// the four probe clips, the health bar's fill clip (its
    /// `_currentframe` is the fill level — Empty vs 75% vs Full),
    /// the location text field (its `text` answers whether
    /// `SetText` landed), the three Meter objects (their
    /// `CurrentPercent`/`TargetPercent`/`Empty`/`Full` answer
    /// whether the fill tween runs), and the compass as a control
    /// (a clip known to render).
    pub const STATE_PATHS: [&'static str; 11] = [
        "HUDMovieBaseInstance.Health",
        "HUDMovieBaseInstance.Magica",
        "HUDMovieBaseInstance.Stamina",
        "HUDMovieBaseInstance.LocationLockBase.LocationNameBase",
        "HUDMovieBaseInstance.Health.HealthMeter_mc.HealthLeft",
        "HUDMovieBaseInstance.Health.HealthMeter_mc",
        "HUDMovieBaseInstance.LocationLockBase.LocationNameBase.LocationTextBase.LocationTextInstance",
        "HUDMovieBaseInstance.HealthMeterLeft",
        "HUDMovieBaseInstance.MagickaMeter",
        "HUDMovieBaseInstance.StaminaMeter",
        "HUDMovieBaseInstance.CompassShoutMeterHolder.Compass",
    ];

    /// Clips whose runtime state the binary dumps in inventory mode:
    /// the lists, the item card (its `_visible` answers whether the
    /// showItemsList flow faded it in), the card's name field (its
    /// `text` answers whether the `RequestItemCardInfo` round trip
    /// populated it — expect "IRON DAGGER"), and the bottom bar.
    pub const INV_STATE_PATHS: [&'static str; 6] = [
        "Menu_mc.InventoryLists_mc",
        "Menu_mc.InventoryLists_mc.ItemsList",
        "Menu_mc.InventoryLists_mc.CategoriesList",
        "Menu_mc.ItemCardFadeHolder_mc.ItemCard_mc",
        "Menu_mc.ItemCardFadeHolder_mc.ItemCard_mc.ItemText.ItemTextField",
        "Menu_mc.BottomBar_mc",
    ];

    /// Read a nested clip's runtime state by dotted path from the
    /// root (M2 diagnostic — vendored `Player::clip_state`).
    pub fn clip_state(&mut self, path: &str) -> String {
        self.player.lock().expect("player lock").clip_state(path)
    }
}

/// The movie's base URL for relative imports (M4): the canonicalized
/// base path joined with the movie's name, as a `file://` URL. The
/// movie itself comes from bytes (BSA), so the file need not exist on
/// disk — only the base directory must. Falls back to a bare
/// `file:///<name>` URL when the base path cannot be resolved (the
/// HUD imports nothing but the fontlib, which the navigator
/// intercepts, so the fallback is safe).
fn movie_url(base_path: &std::path::Path, movie_name: &str) -> String {
    let base = base_path
        .canonicalize()
        .unwrap_or_else(|_| base_path.to_path_buf());
    match url::Url::from_file_path(base.join(movie_name)) {
        Ok(url) => url.to_string(),
        Err(()) => format!("file:///{movie_name}"),
    }
}

/// One inventory-list entry: the fields the list renderer reads
/// (`text`, `count` — appended as " (N)" when > 1) plus the
/// `filterFlag` bitmask the category filter matches on and the
/// `enabled` flag item selection checks.
fn inventory_entry(text: &str, count: f64, filter_flag: f64) -> Value {
    let mut entry = BTreeMap::new();
    entry.insert("text".to_string(), Value::String(text.to_string()));
    entry.insert("count".to_string(), Value::Number(count));
    entry.insert("filterFlag".to_string(), Value::Number(filter_flag));
    entry.insert("enabled".to_string(), Value::Bool(true));
    Value::Object(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_match_the_study() {
        let config = HarnessConfig::new("hudmenu.gfx".into(), "gfxfontlib.swf".into());
        assert_eq!(config.width, 1280);
        assert_eq!(config.height, 720);
        assert_eq!(config.frame_rate, 30.0);
    }

    #[test]
    fn summarize_empty_is_zero() {
        let summary = summarize(&[]);
        assert_eq!(summary.frames, 0);
        assert_eq!(summary.run_mean_ms, 0.0);
    }

    #[test]
    fn summarize_reports_mean_p95_max() {
        let timings: Vec<FrameTiming> = (1..=100)
            .map(|i| FrameTiming {
                frame: i,
                preload_run_ms: i as f64,
                render_ms: 0.5,
            })
            .collect();
        let summary = summarize(&timings);
        assert_eq!(summary.frames, 100);
        assert!((summary.run_mean_ms - 50.5).abs() < 1e-9);
        assert_eq!(summary.run_p95_ms, 96.0);
        assert_eq!(summary.run_max_ms, 100.0);
        assert!((summary.render_mean_ms - 0.5).abs() < 1e-9);
        assert_eq!(summary.render_max_ms, 0.5);
    }
}
