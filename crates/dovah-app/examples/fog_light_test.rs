//! Fog translucency diagnostic: renders the waterfall fog NIF
//! (`meshes/effects/fxwaterfallskirttallfront.nif`) alone, in
//! front of a two-tone dark backdrop, using the CURRENT
//! production material rules (scene.rs `real_material`: blend +
//! unlit for effect shapes). The exterior render's white band
//! still reads as a flat slab — this isolates whether the fog
//! meshes themselves blend (backdrop tones show through) or
//! render opaque (flat white).
//!
//! Usage: `cargo run -p dovah-app --example fog_light_test`
//! Output: `screenshots/fog-light-test.png`
//!
//! The backdrop is two boxes: very dark (0.05) on the left, mid
//! grey (0.5) on the right. If the fog over the dark half reads
//! darker than over the light half, it is translucent.

use bevy::app::{App, AppExit, PluginGroup, Startup, Update};
use bevy::asset::{Assets, RenderAssetUsages};
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
use bevy::shape::Cuboid;
use bevy::time::Time;
use bevy::transform::components::Transform;
use bevy::window::{ExitCondition, WindowPlugin};
use bevy::winit::WinitPlugin;
use bevy::DefaultPlugins;
use dovah_app::convert;
use dovah_bsa::BsaArchive;
use dovah_nif::AlphaBlend;
use std::collections::HashMap;
use std::fs::{self, File};
use std::time::Duration;

fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// One fog sub-mesh with the full `real_material` input.
struct FogMesh {
    mesh: Mesh,
    diffuse: Option<String>,
    gradient: Option<String>,
    cutoff: Option<f32>,
    blend: AlphaBlend,
    unlit: bool,
}

/// The fog's meshes + textures, loaded from the test-data BSAs
/// with the production conversion path.
struct Fog {
    meshes: Vec<FogMesh>,
    textures: HashMap<String, Vec<u8>>,
}

fn load_fog() -> Fog {
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
        read("meshes\\effects\\fxwaterfallskirttallfront.nif").expect("fog NIF in Meshes BSAs");
    let nif = dovah_nif::NifFile::from_bytes(&nif_bytes).expect("parse fog NIF");
    let mut meshes = Vec::new();
    let mut textures = HashMap::new();
    for sub in nif.meshes() {
        println!(
            "fog_light_test: mesh '{}' cutoff={:?} blend={:?} unlit={} diffuse={:?} gradient={:?}",
            sub.name,
            sub.alpha_cutoff,
            sub.alpha_blend,
            sub.unlit,
            sub.textures.diffuse,
            sub.textures.gradient
        );
        let Some(bevy) = convert::nif_mesh_to_bevy(&sub) else {
            continue;
        };
        // Bevy-space AABB: where does this mesh actually sit?
        // (The NIF-local bounds don't include the parent chain.)
        if let Some(bevy::mesh::VertexAttributeValues::Float32x3(pos)) =
            bevy.attribute(Mesh::ATTRIBUTE_POSITION)
        {
            let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in pos {
                for i in 0..3 {
                    mn[i] = mn[i].min(p[i]);
                    mx[i] = mx[i].max(p[i]);
                }
            }
            println!(
                "fog_light_test: mesh '{}' Bevy AABB min=({:.1},{:.1},{:.1}) max=({:.1},{:.1},{:.1})",
                sub.name, mn[0], mn[1], mn[2], mx[0], mx[1], mx[2]
            );
        }
        // UV range: a mesh with no UV attribute (or degenerate
        // UVs) samples a single texel and reads perfectly flat
        // — the first suspect for a textureless sheet.
        match bevy.attribute(Mesh::ATTRIBUTE_UV_0) {
            Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) => {
                let (mut mn, mut mx) = ([f32::MAX; 2], [f32::MIN; 2]);
                for uv in uvs {
                    for i in 0..2 {
                        mn[i] = mn[i].min(uv[i]);
                        mx[i] = mx[i].max(uv[i]);
                    }
                }
                println!(
                    "fog_light_test: mesh '{}' UV_0 {} verts range U [{:.2},{:.2}] V [{:.2},{:.2}]",
                    sub.name,
                    uvs.len(),
                    mn[0],
                    mx[0],
                    mn[1],
                    mx[1]
                );
            }
            _ => println!("fog_light_test: mesh '{}' has NO UV_0 attribute", sub.name),
        }
        if let Some(diffuse) = &sub.textures.diffuse {
            if !textures.contains_key(diffuse) {
                if let Some(bytes) = read(diffuse) {
                    textures.insert(diffuse.clone(), bytes);
                }
            }
        }
        if let Some(gradient) = &sub.textures.gradient {
            if !textures.contains_key(gradient) {
                if let Some(bytes) = read(gradient) {
                    textures.insert(gradient.clone(), bytes);
                }
            }
        }
        meshes.push(FogMesh {
            mesh: bevy,
            diffuse: sub.textures.diffuse.clone(),
            gradient: sub.textures.gradient.clone(),
            cutoff: sub.alpha_cutoff,
            blend: sub.alpha_blend,
            unlit: sub.unlit,
        });
    }
    Fog { meshes, textures }
}

fn main() {
    let fog = load_fog();
    println!("fog_light_test: {} sub-meshes", fog.meshes.len());
    let mut app = App::new();
    app.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 2400.0,
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
    target.asset_usage = RenderAssetUsages::RENDER_WORLD;
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    app.insert_resource(Target(handle))
        .insert_resource(FogRes(fog))
        .add_systems(Startup, setup)
        .add_systems(Update, take_shot);
    app.run();
}

#[derive(Resource)]
struct Target(bevy::asset::Handle<Image>);

#[derive(Resource)]
struct FogRes(Fog);

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    target: Res<Target>,
    fog: Res<FogRes>,
) {
    // Materials: the production rules (scene.rs real_material),
    // INCLUDING the blend + unlit + gradient branches.
    for sub in &fog.0.meshes {
        let mut material = StandardMaterial::default();
        let mut textured = false;
        if let Some(path) = &sub.diffuse {
            if let Some(bytes) = fog.0.textures.get(path) {
                let image = match &sub.gradient {
                    Some(grad_path) => match fog.0.textures.get(grad_path) {
                        Some(grad_bytes) => convert::dds_multiply_to_bevy_image(bytes, grad_bytes),
                        None => convert::dds_to_bevy_image(bytes),
                    },
                    None => convert::dds_to_bevy_image(bytes),
                };
                if let Some(image) = image {
                    material.base_color_texture = Some(images.add(image));
                    textured = true;
                }
            }
        }
        material.base_color = if textured || sub.blend != AlphaBlend::Opaque {
            Color::WHITE
        } else {
            Color::srgb(0.85, 0.85, 0.85)
        };
        if sub.unlit {
            material.unlit = true;
        }
        if textured && sub.cutoff.is_some() {
            material.alpha_mode = AlphaMode::AlphaToCoverage;
            material.double_sided = true;
        } else if sub.blend != AlphaBlend::Opaque {
            material.alpha_mode = match sub.blend {
                AlphaBlend::Blend => AlphaMode::Blend,
                AlphaBlend::Additive => AlphaMode::Add,
                AlphaBlend::Opaque => AlphaMode::Opaque,
            };
            material.double_sided = true;
        }
        commands.spawn((
            Mesh3d(meshes.add(sub.mesh.clone())),
            MeshMaterial3d(materials.add(material)),
            Transform::IDENTITY,
        ));
    }
    // Backdrop: two boxes behind the fog sheet (the sheet sits
    // near Bevy z=+7, facing -Z). Dark left, mid-grey right —
    // the translucency test is whether the two halves read
    // differently THROUGH the fog. Centred at y=25 so they cover
    // the sheet's opaque top (y 29..47), which is the part the
    // village sees as the slab.
    let mut backdrop = |x: f32, shade: f32| {
        (
            Mesh3d(meshes.add(Mesh::from(Cuboid::new(30.0, 50.0, 1.0)))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(shade, shade, shade),
                unlit: true,
                ..Default::default()
            })),
            Transform::from_translation(Vec3::new(x, 25.0, 20.0)),
        )
    };
    commands.spawn(backdrop(-11.0, 0.05));
    commands.spawn(backdrop(19.0, 0.5));
    // Sun: the exterior's sun, same position and color.
    let sun_transform =
        Transform::from_translation(Vec3::new(100.0, 150.0, 100.0)).looking_at(Vec3::ZERO, Vec3::Y);
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            color: Color::srgb(1.0, 0.96, 0.88),
            shadow_maps_enabled: false,
            ..Default::default()
        },
        sun_transform,
    ));
    // Camera: the newFog sheet spans Bevy x [-9, 8], y [0, 47]
    // at z ~+7, and its opacity lives at the TOP (the vertex
    // alpha fades from ~0 at the bottom to ~0.8 at the top —
    // the first framing, looking at y=8, saw only the
    // transparent belly and the sheet was invisible). Frame the
    // top, the part the village sees as the slab.
    commands.spawn((
        Camera3d::default(),
        RenderTarget::Image(target.0.clone().into()),
        Exposure { ev100: 12.0 },
        Tonemapping::AgX,
        Transform::from_translation(Vec3::new(4.0, 30.0, -30.0))
            .looking_at(Vec3::new(4.0, 26.0, 7.0), Vec3::Y),
    ));
}

fn take_shot(mut commands: Commands, target: Res<Target>, time: Res<Time>, mut done: Local<bool>) {
    if *done || time.elapsed_secs() < 1.0 {
        return;
    }
    *done = true;
    let path = "screenshots/fog-light-test.png".to_string();
    println!("fog_light_test: capturing to {path}");
    commands
        .spawn(Screenshot::image(target.0.clone()))
        .observe(save_to_disk(path))
        .observe(exit_after);
}

fn exit_after(_event: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>) {
    exit.write(AppExit::Success);
}
