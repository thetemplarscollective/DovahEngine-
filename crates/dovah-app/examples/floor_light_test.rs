//! Normal-map flip diagnostic: renders ONLY the inn floor
//! (shape :16 of `meshes/architecture/farmhouse/interior/
//! farmintinnend01.nif`, StoneFloor01 + StoneFloor01_n) under a
//! single raking point light, with Bevy's DirectX Y flip on or
//! off. The mortar joints are RECESSED grooves: in the correct
//! render the joint's light-facing wall is bright and the far
//! wall is dark; a flipped channel swaps bright/dark. The light
//! rakes diagonally so BOTH joint orientations (U = red, V =
//! green) read in one shot.
//!
//! Usage: `cargo run -p dovah-app --example floor_light_test --
//!        <tag> [--no-flip] [--flip-red]`
//! Output: `screenshots/floor-light-test-<tag>.png`
//!
//! `--no-flip` disables `flip_normal_map_y` (the production
//! default is ON — Skyrim's maps are DirectX, green down).
//! `--flip-red` inverts the normal map's R channel at load
//! (DIAGNOSTIC ONLY — tests whether red is also negated).

use bevy::app::{App, AppExit, PluginGroup, Startup, Update};
use bevy::asset::{Assets, RenderAssetUsages};
use bevy::camera::{Camera3d, ClearColor, Exposure, RenderTarget};
use bevy::color::Color;
use bevy::core_pipeline::tonemapping::Tonemapping;
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
use bevy::time::Time;
use bevy::transform::components::Transform;
use bevy::window::{ExitCondition, WindowPlugin};
use bevy::winit::WinitPlugin;
use bevy::DefaultPlugins;
use dovah_app::convert;
use dovah_bsa::BsaArchive;
use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::time::Duration;

fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

struct Floor {
    mesh: Mesh,
    diffuse: Option<Vec<u8>>,
    normal: Option<Vec<u8>>,
}

fn load_floor() -> Floor {
    let mut archives: Vec<BsaArchive<File>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut paths: Vec<_> = fs::read_dir("test-data")
        .expect("read data dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .map(|e| e.eq_ignore_ascii_case("bsa"))
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    for path in paths {
        if let Ok(archive) = BsaArchive::open(&path) {
            let idx = archives.len();
            for stored in archive.list() {
                index.entry(normalize(stored)).or_insert(idx);
            }
            archives.push(archive);
        }
    }
    let mut read = |path: &str| -> Option<Vec<u8>> {
        let idx = *index.get(&normalize(path))?;
        let stored: String = archives[idx]
            .list()
            .into_iter()
            .find(|s| normalize(s) == normalize(path))?
            .to_string();
        archives[idx].extract(&stored).ok()
    };
    let nif_bytes = read("meshes\\architecture\\farmhouse\\interior\\farmintinnend01.nif")
        .expect("floor NIF in Meshes BSAs");
    let nif = dovah_nif::NifFile::from_bytes(&nif_bytes).expect("parse floor NIF");
    // Shape :16 is the floor (StoneFloor01, flat at Z=0).
    let sub = nif
        .meshes()
        .into_iter()
        .find(|m| m.name.ends_with(":16"))
        .expect("floor shape :16");
    println!(
        "floor_light_test: shape '{}' diffuse={:?} normal={:?} verts={}",
        sub.name,
        sub.textures.diffuse,
        sub.textures.normal,
        sub.vertices.len()
    );
    let bevy = convert::nif_mesh_to_bevy(&sub).expect("floor converts");
    let diffuse = sub.textures.diffuse.as_deref().and_then(&mut read);
    let normal = sub.textures.normal.as_deref().and_then(&mut read);
    println!(
        "floor_light_test: diffuse {} bytes, normal {} bytes",
        diffuse.as_ref().map_or(0, Vec::len),
        normal.as_ref().map_or(0, Vec::len)
    );
    assert!(normal.is_some(), "normal map must load for this test");
    Floor {
        mesh: bevy,
        diffuse,
        normal,
    }
}

#[derive(Resource)]
struct Target(bevy::asset::Handle<Image>);

#[derive(Resource)]
struct FloorRes(Floor);

#[derive(Resource)]
struct Shot {
    tag: String,
    flip: bool,
    flip_red: bool,
}

fn main() {
    let mut args = env::args().skip(1);
    let tag = args.next().unwrap_or_else(|| "flip".to_string());
    let rest: Vec<String> = args.collect();
    let flip = !rest.iter().any(|a| a == "--no-flip");
    let flip_red = rest.iter().any(|a| a == "--flip-red");
    let floor = load_floor();
    let mut app = App::new();
    app.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 10.0,
        ..Default::default()
    })
    .insert_resource(ClearColor(Color::srgb(0.08, 0.09, 0.12)))
    .add_plugins(
        DefaultPlugins
            .build()
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
    let mut target = Image::new_target_texture(1280, 720, TextureFormat::Rgba8UnormSrgb, None);
    target.asset_usage = RenderAssetUsages::RENDER_WORLD;
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    app.insert_resource(Target(handle))
        .insert_resource(FloorRes(floor))
        .insert_resource(Shot {
            tag,
            flip,
            flip_red,
        })
        .add_systems(Startup, setup)
        .add_systems(Update, take_shot);
    app.run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    target: Res<Target>,
    floor: Res<FloorRes>,
    shot: Res<Shot>,
) {
    // Material: the production rules (scene.rs real_material)
    // for an opaque textured shape with a normal map.
    let mut material = StandardMaterial::default();
    if let Some(bytes) = &floor.0.diffuse {
        if let Some(image) = convert::dds_to_bevy_image(bytes) {
            material.base_color_texture = Some(images.add(image));
        }
    }
    if let Some(bytes) = &floor.0.normal {
        if let Some(mut image) = convert::dds_normal_to_bevy_image(bytes) {
            if shot.flip_red {
                // Diagnostic only: invert R (X) to test whether
                // the red channel is negated. Z is left as-is;
                // the shader normalizes.
                if let Some(data) = image.data.as_mut() {
                    for px in data.as_chunks_mut::<4>().0 {
                        px[0] = 255 - px[0];
                    }
                }
            }
            material.normal_map_texture = Some(images.add(image));
            material.flip_normal_map_y = shot.flip;
        }
    }
    material.base_color = Color::WHITE;
    material.perceptual_roughness = 1.0;
    commands.spawn((
        Mesh3d(meshes.add(floor.0.mesh.clone())),
        MeshMaterial3d(materials.add(material)),
        Transform::IDENTITY,
    ));
    // Raking point light, diagonal (+X, +Z) and low, so both
    // joint orientations shade. Warm, inn-like.
    commands.spawn((
        PointLight {
            intensity: 100_000.0,
            color: Color::srgb(1.0, 0.85, 0.6),
            range: 30.0,
            shadow_maps_enabled: false,
            ..Default::default()
        },
        Transform::from_translation(Vec3::new(3.3, 2.0, 4.0)),
    ));
    // Camera: the floor spans Bevy x [-3.7, 2.2], z [-5.9, 5.9]
    // at y=0. Look down at ~45 deg from the opposite diagonal
    // so the light rakes across the frame.
    commands.spawn((
        Camera3d::default(),
        RenderTarget::Image(target.0.clone().into()),
        Exposure { ev100: 7.0 },
        Tonemapping::AgX,
        Transform::from_translation(Vec3::new(-4.5, 5.5, -6.5))
            .looking_at(Vec3::new(-0.7, 0.0, 0.0), Vec3::Y),
    ));
}

fn take_shot(
    mut commands: Commands,
    target: Res<Target>,
    shot: Res<Shot>,
    time: Res<Time>,
    mut done: Local<bool>,
) {
    if *done || time.elapsed_secs() < 1.0 {
        return;
    }
    *done = true;
    let path = format!("screenshots/floor-light-test-{}.png", shot.tag);
    println!("floor_light_test: capturing to {path} (flip={})", shot.flip);
    commands
        .spawn(Screenshot::image(target.0.clone()))
        .observe(save_to_disk(path))
        .observe(exit_after);
}

fn exit_after(_event: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>) {
    exit.write(AppExit::Success);
}
