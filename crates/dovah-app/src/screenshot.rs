//! Headless screenshot mode: render the scene to an image target
//! and save one frame as a PNG, then exit.
//!
//! Uses Bevy's built-in [`Screenshot`] component plus the
//! `save_to_disk` observer — the documented way to capture a single
//! frame, simpler than the manual render-graph copy in the
//! `headless_renderer` example. The app runs headless (no window,
//! `ScheduleRunnerPlugin` driving the loop), so it works on a VM
//! with no display as long as a Vulkan device exists (lavapipe, the
//! software driver, is enough).

use bevy::app::{App, AppExit, Plugin, Update};
use bevy::asset::Handle;
use bevy::ecs::prelude::*;
use bevy::image::Image;
use bevy::prelude::in_state;
use bevy::prelude::State;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::window::screenshot::{save_to_disk, Screenshot, ScreenshotCaptured};

/// The render-target image the screenshot camera draws into.
#[derive(Resource, Clone)]
pub struct ScreenshotTarget {
    pub image: Handle<Image>,
}

/// Screenshot configuration (headless mode only).
#[derive(Resource)]
pub struct ScreenshotConfig {
    pub path: String,
    /// Frames to render AFTER the scene build finishes
    /// (InGame) before capturing (lets meshes upload and the
    /// scene settle). The chunked build pumps Spawning for as
    /// many frames as it takes first — `--frames` is the
    /// post-build settle count, not a delay from startup.
    pub pre_roll_frames: u32,
    captured: bool,
}

impl ScreenshotConfig {
    pub fn new(path: String, pre_roll_frames: u32) -> Self {
        ScreenshotConfig {
            path,
            pre_roll_frames,
            captured: false,
        }
    }
}

pub struct ScreenshotPlugin;

impl Plugin for ScreenshotPlugin {
    fn build(&self, app: &mut App) {
        // The target image is NOT created here: main.rs creates it
        // directly on the App (before any system runs) so the
        // `ScreenshotTarget` resource already exists when
        // `spawn_camera` reads it. Doing it in a Startup system via
        // `Commands` would NOT work — commands apply after the whole
        // Startup schedule, so a chained camera system would see no
        // target and render to the (nonexistent) primary window.
        app.add_systems(Update, take_screenshot)
            // Water flow (water.rs): the screenshot path has
            // no player plugin, so this path registers the
            // scroll here — PlayerPlugin carries the same line
            // for the windowed path.
            .add_systems(
                Update,
                crate::water::animate_water_flow.run_if(in_state(crate::AppState::InGame)),
            )
            // Mill wheel (mill.rs): same split — PlayerPlugin
            // carries the same line for the windowed path.
            .add_systems(
                Update,
                crate::mill::animate_mill_wheel.run_if(in_state(crate::AppState::InGame)),
            )
            // Sawmill blade (sawmill.rs): same split.
            .add_systems(
                Update,
                crate::sawmill::animate_sawmill_blade.run_if(in_state(crate::AppState::InGame)),
            );
    }
}

/// Build the render-target image (same shape as Bevy's
/// `externally_driven_headless_renderer` example: sRGB target,
/// render-world only).
pub fn create_target_image(width: u32, height: u32) -> Image {
    let mut target = Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None);
    target.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
    target
}

fn take_screenshot(
    mut commands: Commands,
    mut config: ResMut<ScreenshotConfig>,
    target: Res<ScreenshotTarget>,
    state: Res<State<crate::AppState>>,
    mut frames: Local<u32>,
) {
    if config.captured {
        return;
    }
    // The scene builds chunked in Spawning — count only InGame
    // frames (the post-build settle), so the capture never
    // catches a half-built scene.
    if *state.get() != crate::AppState::InGame {
        return;
    }
    *frames += 1;
    if *frames < config.pre_roll_frames {
        return;
    }
    config.captured = true;
    let frames_done = *frames;
    println!(
        "capturing screenshot to {} after {frames_done} frames",
        config.path
    );
    commands
        .spawn(Screenshot::image(target.image.clone()))
        .observe(save_to_disk(config.path.clone()))
        .observe(exit_after_screenshot);
}

/// Once the screenshot is captured (and saved by the other
/// observer), shut the app down cleanly.
fn exit_after_screenshot(_event: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>) {
    exit.write(AppExit::Success);
}
