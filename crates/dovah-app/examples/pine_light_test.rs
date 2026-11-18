//! Exterior follow-up diagnostic: ONE pine under a controlled
//! light rig, so the foliage darkness can be measured knob by knob
//! instead of guessed at from full exterior renders (each of which
//! costs a 2.4 GB scene build).
//!
//! The rig reproduces the exterior exactly at baseline: sun 12,000
//! lux from (100,150,100), flat white ambient 800, OVERCAST
//! (ev100 12) exposure, AgX tonemapping, the real NIF->Bevy
//! conversion (`dovah_app::convert`) and the real material rules
//! (A2C + double-sided for cutout foliage, opaque trunk).
//!
//! Usage: cargo run -p dovah-app --example pine_light_test --
//!        <tag> [--ambient N] [--sun N]
//!        [--tonemapping agx|none|reinhard|blender|aces]
//!        [--exposure EV] [--camside shade|sun] [--vcolor on|off]
//! Output: screenshots/pine-light-test-<tag>.png
//!
//! Measure the foliage afterwards with PIL (crop the tree, mean /
//! p5 / p95) — the numbers, not the eye, decide which knob moves.

use bevy::app::{App, AppExit, PluginGroup, Startup, Update};
use bevy::asset::Assets;
use bevy::camera::{Camera3d, ClearColor, Exposure, RenderTarget};
use bevy::color::Color;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::ecs::prelude::*;
use bevy::image::Image;
use bevy::light::{DirectionalLight, GlobalAmbientLight};
use bevy::material::AlphaMode;
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
use dovah_bsa::BsaArchive;
use std::collections::HashMap;
use std::fs::{self, File};
use std::time::Duration;

#[derive(Clone, Resource)]
struct Opts {
    tag: String,
    ambient: f32,
    sun: f32,
    tonemapping: String,
    exposure: f32,
    camside: String,
    vcolor: bool,
}

fn parse_opts() -> Opts {
    let mut o = Opts {
        tag: "base".to_string(),
        ambient: 800.0,
        sun: 12_000.0,
        tonemapping: "agx".to_string(),
        exposure: 12.0,
        camside: "shade".to_string(),
        vcolor: true,
    };
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--ambient" => {
                o.ambient = raw[i + 1].parse().unwrap();
                i += 2;
            }
            "--sun" => {
                o.sun = raw[i + 1].parse().unwrap();
                i += 2;
            }
            "--tonemapping" => {
                o.tonemapping = raw[i + 1].clone();
                i += 2;
            }
            "--exposure" => {
                o.exposure = raw[i + 1].parse().unwrap();
                i += 2;
            }
            "--camside" => {
                o.camside = raw[i + 1].clone();
                i += 2;
            }
            "--vcolor" => {
                o.vcolor = raw[i + 1] == "on";
                i += 2;
            }
            other => {
                if !other.starts_with("--") {
                    o.tag = other.to_string();
                }
                i += 1;
            }
        }
    }
    o
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// The pine's meshes + textures, loaded from the test-data BSAs
/// with the production conversion path.
struct Pine {
    meshes: Vec<(Mesh, Option<String>, Option<f32>)>,
    textures: HashMap<String, Vec<u8>>,
}

fn load_pine(vcolor: bool) -> Pine {
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
    let nif_bytes =
        read("meshes\\landscape\\trees\\treePineForest01.nif").expect("pine NIF in Meshes BSAs");
    let nif = dovah_nif::NifFile::from_bytes(&nif_bytes).expect("parse pine NIF");
    let mut meshes = Vec::new();
    let mut textures = HashMap::new();
    for sub in nif.meshes() {
        let Some(mut bevy) = dovah_app::convert::nif_mesh_to_bevy(&sub) else {
            continue;
        };
        if !vcolor {
            bevy.remove_attribute(Mesh::ATTRIBUTE_COLOR);
        }
        if let Some(diffuse) = &sub.textures.diffuse {
            if !textures.contains_key(diffuse) {
                if let Some(bytes) = read(diffuse) {
                    textures.insert(diffuse.clone(), bytes);
                }
            }
        }
        meshes.push((bevy, sub.textures.diffuse.clone(), sub.alpha_cutoff));
    }
    Pine { meshes, textures }
}

fn main() {
    let opts = parse_opts();
    println!(
        "pine_light_test: tag={} ambient={} sun={} tonemapping={} exposure={} camside={} vcolor={}",
        opts.tag,
        opts.ambient,
        opts.sun,
        opts.tonemapping,
        opts.exposure,
        opts.camside,
        opts.vcolor
    );
    let pine = load_pine(opts.vcolor);
    println!("pine_light_test: {} sub-meshes", pine.meshes.len());
    let mut app = App::new();
    app.insert_resource(GlobalAmbientLight {
        // M1: sky-blue fill, matching EXTERIOR_AMBIENT_COLOR
        // in scene.rs (was WHITE — half the grey wash).
        color: Color::srgb(0.75, 0.86, 0.96),
        brightness: opts.ambient,
        ..Default::default()
    })
    .insert_resource(ClearColor(Color::srgb(0.53, 0.66, 0.82)))
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
    let mut target = Image::new_target_texture(1280, 720, TextureFormat::Rgba8UnormSrgb, None);
    target.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    app.insert_resource(Target(handle))
        .insert_resource(opts)
        .insert_resource(PineRes(pine))
        .add_systems(Startup, setup)
        .add_systems(Update, take_shot);
    app.run();
}

#[derive(Resource)]
struct Target(bevy::asset::Handle<Image>);

#[derive(Resource)]
struct PineRes(Pine);

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    target: Res<Target>,
    opts: Res<Opts>,
    pine: Res<PineRes>,
) {
    // Materials: the production rules (scene.rs real_material).
    for (mesh, diffuse, cutoff) in &pine.0.meshes {
        let mut material = StandardMaterial::default();
        let mut textured = false;
        if let Some(path) = diffuse {
            if let Some(bytes) = pine.0.textures.get(path) {
                if let Some(image) = dovah_app::convert::dds_to_bevy_image(bytes) {
                    material.base_color_texture = Some(images.add(image));
                    textured = true;
                }
            }
        }
        material.base_color = if textured {
            Color::WHITE
        } else {
            Color::srgb(0.85, 0.85, 0.85)
        };
        // M1: match the production rules in real_material —
        // flat matte roughness + binary Mask at the NIF
        // threshold (was Bevy default 0.5 + A2C).
        material.perceptual_roughness = 0.9;
        if textured && cutoff.is_some() {
            material.alpha_mode = AlphaMode::Mask((*cutoff).unwrap());
            material.double_sided = true;
        }
        commands.spawn((
            Mesh3d(meshes.add(mesh.clone())),
            MeshMaterial3d(materials.add(material)),
            Transform::IDENTITY,
        ));
    }
    // Sun: the exterior's sun, same position and color.
    let sun_transform =
        Transform::from_translation(Vec3::new(100.0, 150.0, 100.0)).looking_at(Vec3::ZERO, Vec3::Y);
    commands.spawn((
        DirectionalLight {
            illuminance: opts.sun,
            color: Color::srgb(1.0, 0.96, 0.88),
            shadow_maps_enabled: false,
            ..Default::default()
        },
        sun_transform,
    ));
    // Camera: the tree is ~21 m tall at the origin. Shade side
    // (NW) sees the backlit face — the M13 shot. Sun side (SE)
    // sees the lit face.
    let (pos, look) = if opts.camside == "sun" {
        (Vec3::new(8.0, 4.0, 10.0), Vec3::new(0.0, 8.0, 0.0))
    } else {
        (Vec3::new(-8.0, 4.0, -10.0), Vec3::new(0.0, 8.0, 0.0))
    };
    let tonemapping = match opts.tonemapping.as_str() {
        "none" => Tonemapping::None,
        "reinhard" => Tonemapping::Reinhard,
        "blender" => Tonemapping::BlenderFilmic,
        "aces" => Tonemapping::AcesFitted,
        _ => Tonemapping::AgX,
    };
    commands.spawn((
        Camera3d::default(),
        RenderTarget::Image(target.0.clone().into()),
        Exposure {
            ev100: opts.exposure,
        },
        tonemapping,
        Transform::from_translation(pos).looking_at(look, Vec3::Y),
    ));
}

fn take_shot(
    mut commands: Commands,
    target: Res<Target>,
    opts: Res<Opts>,
    time: Res<Time>,
    mut done: Local<bool>,
) {
    if *done || time.elapsed_secs() < 1.0 {
        return;
    }
    *done = true;
    let path = format!("screenshots/pine-light-test-{}.png", opts.tag);
    println!("pine_light_test: capturing to {path}");
    commands
        .spawn(Screenshot::image(target.0.clone()))
        .observe(save_to_disk(path))
        .observe(exit_after);
}

fn exit_after(_event: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>) {
    exit.write(AppExit::Success);
}
