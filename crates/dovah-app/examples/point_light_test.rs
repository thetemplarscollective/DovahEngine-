//! M14 diagnostic: point-light response curve in this
//! headless/lavapipe setup.
//!
//! One white cube lit by one warm point light 2 m away (range
//! 20), ZERO ambient — the point light is the only light.
//! The intensity comes from argv[1] (default 2000 cd) so the
//! response curve can be measured across runs WITHOUT cross-talk
//! (a multi-light scene proved useless: the 20000 cd light lit
//! every cube, including the "control").
//! Measures the true brightness-vs-intensity response so the
//! inn's light intensity can be calibrated from data, not from
//! the physical-units model (which the first run showed to be
//! ~7x optimistic).
//!
//! Usage: cargo run -p dovah-app --example point_light_test -- <intensity>
//! Output: screenshots/point-light-test-<intensity>.png

use bevy::app::{App, AppExit, PluginGroup, Startup, Update};
use bevy::asset::Assets;
use bevy::camera::{Camera3d, ClearColor, RenderTarget};
use bevy::color::Color;
use bevy::ecs::prelude::*;
use bevy::image::Image;
use bevy::light::{GlobalAmbientLight, PointLight};
use bevy::math::Vec3;
use bevy::mesh::Mesh;
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::render::mesh::Mesh3d;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::window::screenshot::{save_to_disk, Screenshot, ScreenshotCaptured};
use bevy::render::RenderPlugin;
use bevy::shape::Cuboid;
use bevy::time::Time;
use bevy::transform::components::Transform;
use bevy::window::{ExitCondition, WindowPlugin};
use bevy::winit::WinitPlugin;
use bevy::DefaultPlugins;
use std::time::Duration;

fn main() {
    let mut app = App::new();
    // Zero ambient: the point light is the ONLY light.
    app.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 0.0,
        ..Default::default()
    })
    .insert_resource(ClearColor(Color::BLACK))
    .add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..Default::default()
            })
            .set(RenderPlugin {
                synchronous_pipeline_compilation: true,
                ..Default::default()
            })
            .disable::<WinitPlugin>(),
    )
    .add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
        Duration::from_secs_f64(1.0 / 30.0),
    ));
    // Render target, created on the App before any system runs.
    let mut target = Image::new_target_texture(640, 360, TextureFormat::Rgba8UnormSrgb, None);
    target.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    app.insert_resource(Target(handle))
        .add_systems(Startup, setup)
        .add_systems(Update, take_shot);
    app.run();
}

#[derive(Resource)]
struct Target(bevy::asset::Handle<Image>);

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    target: Res<Target>,
) {
    let cube = meshes.add(Mesh::from(Cuboid::new(1.0, 1.0, 1.0)));
    let white = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 1.0,
        ..Default::default()
    });
    let intensity = std::env::args()
        .nth(1)
        .and_then(|a| a.parse::<f32>().ok())
        .unwrap_or(2000.0);
    println!("point_light_test: intensity {intensity} cd");
    // White cube at the origin, 1 m across.
    commands.spawn((
        Mesh3d(cube),
        MeshMaterial3d(white),
        Transform::from_translation(Vec3::ZERO),
    ));
    // Warm point light 2 m in front of the cube, 1 m up.
    commands.spawn((
        PointLight {
            intensity,
            range: 20.0,
            color: Color::srgb(1.0, 0.8, 0.5),
            ..Default::default()
        },
        Transform::from_translation(Vec3::new(0.0, 1.0, 2.0)),
    ));
    // Camera 4 m back, looking at the cube.
    commands.spawn((
        Camera3d::default(),
        RenderTarget::Image(target.0.clone().into()),
        Transform::from_translation(Vec3::new(0.0, 0.5, 4.0)).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn take_shot(mut commands: Commands, target: Res<Target>, time: Res<Time>, mut done: Local<bool>) {
    // Wait ~1 s of frames for pipelines + uploads to settle.
    if *done || time.elapsed_secs() < 1.0 {
        return;
    }
    *done = true;
    let intensity = std::env::args()
        .nth(1)
        .and_then(|a| a.parse::<f32>().ok())
        .unwrap_or(2000.0);
    let path = format!("screenshots/point-light-test-{intensity}.png");
    println!("point_light_test: capturing to {path}");
    commands
        .spawn(Screenshot::image(target.0.clone()))
        .observe(save_to_disk(path))
        .observe(exit_after);
}

fn exit_after(_event: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>) {
    exit.write(AppExit::Success);
}
