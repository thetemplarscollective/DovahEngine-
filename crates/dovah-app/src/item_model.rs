//! The inventory's item model (vanilla): the selected item's
//! ground model rendered into a texture on the right of the
//! inventory screen, rotatable by mouse drag — the game's
//! `MouseRotationInputCatcher` (inventorymenu.swf, at (499.2,
//! 4.3) on the 1280x720 stage). Follows the player_model.rs
//! pattern (own render layer + camera + light), on layer 2
//! (the doll is layer 1, the world is layer 0).
//!
//! The camera FRAMES each item: the model's bounds are measured
//! from its mesh positions and the camera + light are placed to
//! fit, so a sword and a tankard both fill the viewport. The
//! light's intensity is the target irradiance divided by the
//! linear falloff at the light's distance (see
//! `skyrim_falloff`), so every item is lit the same at any
//! model size — the linear-law analogue of the old d^2 rule.
//!
//! The viewport's backdrop is the doll's dark clear color, not
//! the world: the render target cannot show the world through
//! (the 3D pass writes opaque alpha), so a dark panel is the
//! honest backdrop. The game shows the world behind the model —
//! a transparent viewport is a future improvement, not faked
//! here.

use crate::assets::AssetProvider;
use crate::convert;
use crate::inventory::{InventoryOpen, ItemCatalog, ItemDef, SelectedItem};
use bevy::asset::{Assets, Handle};
use bevy::camera::visibility::{RenderLayers, Visibility};
use bevy::camera::{Camera3d, ClearColorConfig, PerspectiveProjection, Projection, RenderTarget};
use bevy::color::Color;
use bevy::ecs::prelude::*;
use bevy::image::Image;
use bevy::light::PointLight;
use bevy::math::{Quat, Vec3};
use bevy::mesh::Mesh;
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::picking::events::PointerDrag;
use bevy::render::mesh::{Mesh3d, VertexAttributeValues};
use bevy::render::render_resource::TextureFormat;
use bevy::transform::components::Transform;
use std::collections::HashMap;

/// The item-model viewport size in stage pixels (the inventory's
/// model viewport is 740x410 — see inventory.rs's layout
/// constants). The render target is built at this size.
pub const ITEM_MODEL_WIDTH: u32 = 740;
pub const ITEM_MODEL_HEIGHT: u32 = 410;
/// The model camera's vertical field of view (the doll's 40°).
pub const ITEM_MODEL_FOV: f32 = 40.0f32.to_radians();
/// Target irradiance at the framed item in lux. The old
/// inverse-square law held the item at ~23,000 lux at any frame
/// distance (intensity = 23,000 x d^2, irradiance = I/d^2);
/// the linear law keeps the same target (intensity = target /
/// falloff), so the item renders identically.
const ITEM_MODEL_TARGET_LUX: f32 = 23_000.0;
/// The item light's range in metres — the falloff cutoff.
const ITEM_MODEL_LIGHT_RANGE: f32 = 6.0;
/// The model's starting yaw: a slight turn off dead-on, so the
/// item reads three-dimensional on open (the game shows items
/// at a slight angle).
pub const DEFAULT_YAW: f32 = 0.35;

/// The render-target image the item camera draws into, shown by
/// the inventory's model viewport.
#[derive(Resource)]
pub struct ItemModelTarget {
    pub image: Handle<Image>,
}

pub fn create_item_model_target(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut target = Image::new_target_texture(
        ITEM_MODEL_WIDTH,
        ITEM_MODEL_HEIGHT,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    target.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
    let handle = images.add(target);
    commands.insert_resource(ItemModelTarget { image: handle });
}

/// Marker for every entity of the item-model rig (root, camera,
/// light, parts) — despawned as a group on close.
#[derive(Component)]
pub struct ItemModelEntity;

/// Marker for the item-model camera (its transform is reframed
/// per item).
#[derive(Component)]
pub struct ItemModelCamera;

/// Marker for the item-model light (its transform + intensity
/// are reframed per item).
#[derive(Component)]
pub struct ItemModelLight;

/// Marker for the yaw parent: the model's parts are its
/// children, so rotating it turns the model (the drag target).
#[derive(Component)]
pub struct ItemModelRoot;

/// Marker for one spawned sub-mesh of the current item's model
/// (despawned on selection change; the rig stays).
#[derive(Component)]
pub struct ItemModelPart;

/// The model's yaw around Y, in radians — set by the drag
/// observer, applied to the root by `apply_item_model_yaw`,
/// reset to `DEFAULT_YAW` on open and on selection change.
#[derive(Debug, Default, Resource)]
pub struct ItemModelYaw(pub f32);

/// The base form ID the rig currently shows (`None` when it
/// shows nothing — no selection, or a model-less item). The
/// rebuild system compares this against the selection instead
/// of `is_changed()`: the rig-spawn and rebuild systems run in
/// unspecified order, and comparing the modeled base makes a
/// double spawn impossible in either order.
#[derive(Debug, Default, Resource)]
pub struct ItemModelSelection(pub Option<u32>);

/// A model's measured bounds: its center and its bounding
/// sphere's radius (half the AABB diagonal).
#[derive(Debug, Clone, Copy)]
pub struct ModelBounds {
    pub center: Vec3,
    pub radius: f32,
}

/// Bounds from an AABB's corners. Pure — unit-tested.
pub fn bounds_from_min_max(min: Vec3, max: Vec3) -> ModelBounds {
    let center = (min + max) * 0.5;
    let radius = (max - min).length() * 0.5;
    ModelBounds { center, radius }
}

/// The camera distance that fits a bounding sphere of `radius`
/// in a perspective frustum of `fov` (vertical), with a 15%
/// margin so the item never touches the viewport's edges.
/// Pure — unit-tested.
pub fn frame_distance(radius: f32, fov: f32) -> f32 {
    radius.max(0.05) / (fov * 0.5).tan() * 1.15
}

/// The camera + light transforms framing `bounds`, and the
/// light's intensity: the camera sits on -Z (the doll's side),
/// slightly above the center, looking at it; the light sits
/// up-right of the model on the camera side. The intensity
/// holds the item's irradiance at ITEM_MODEL_TARGET_LUX at any
/// size (intensity = target / falloff — the linear-law
/// analogue of the old 23,000 x distance^2 rule). Pure —
/// unit-tested.
pub fn frame_transforms(bounds: &ModelBounds) -> (Transform, Transform, f32) {
    let radius = bounds.radius.max(0.05);
    let distance = frame_distance(radius, ITEM_MODEL_FOV);
    let eye = bounds.center + Vec3::new(0.0, 0.25 * radius, -distance);
    let camera = Transform::from_translation(eye).looking_at(bounds.center, Vec3::Y);
    let light_offset = Vec3::new(0.45, 0.7, -0.6) * distance;
    let light = Transform::from_translation(bounds.center + light_offset);
    // Linear falloff: irradiance = intensity x (1 - d/R). The
    // 0.05 floor keeps the intensity finite if a huge item ever
    // pushed the light near the range edge.
    let falloff =
        crate::scene::skyrim_falloff(light_offset.length(), ITEM_MODEL_LIGHT_RANGE, 1.0).max(0.05);
    let intensity = ITEM_MODEL_TARGET_LUX / falloff;
    (camera, light, intensity)
}

/// The item light: warm, like the doll's (the inn-firelight
/// feel), at the framed intensity.
pub fn item_model_light(intensity: f32) -> PointLight {
    PointLight {
        intensity,
        range: ITEM_MODEL_LIGHT_RANGE,
        color: Color::srgb(1.0, 0.85, 0.65),
        ..Default::default()
    }
}

/// Spawn the rig: the yaw root, the camera, and the light, all
/// on layer 2. Returns the (root, camera, light) entities. The
/// camera + light start at a default frame (a 1 m model at the
/// origin) — the parts spawn reframes them for the real item.
pub fn spawn_item_model_rig(
    commands: &mut Commands,
    target: &ItemModelTarget,
) -> (Entity, Entity, Entity) {
    let root = commands
        .spawn((
            Transform::from_rotation(Quat::from_rotation_y(DEFAULT_YAW)),
            // The parts carry RenderLayers + Visibility (the
            // mesh bundle requires Visibility) — the parent
            // must carry them too, or Bevy warns B0004
            // (hierarchy components must agree).
            RenderLayers::layer(2),
            Visibility::default(),
            ItemModelEntity,
            ItemModelRoot,
        ))
        .id();
    let default_bounds = ModelBounds {
        center: Vec3::ZERO,
        radius: 0.5,
    };
    let (camera_transform, light_transform, intensity) = frame_transforms(&default_bounds);
    let camera = commands
        .spawn((
            Camera3d::default(),
            bevy::camera::Camera {
                // The doll's dark backdrop (see the module doc).
                clear_color: ClearColorConfig::Custom(Color::srgb(0.05, 0.05, 0.08)),
                ..Default::default()
            },
            RenderTarget::Image(target.image.clone().into()),
            RenderLayers::layer(2),
            Projection::Perspective(PerspectiveProjection {
                fov: ITEM_MODEL_FOV,
                ..Default::default()
            }),
            camera_transform,
            ItemModelEntity,
            ItemModelCamera,
        ))
        .id();
    let light = commands
        .spawn((
            item_model_light(intensity),
            light_transform,
            RenderLayers::layer(2),
            ItemModelEntity,
            ItemModelLight,
        ))
        .id();
    (root, camera, light)
}

/// The POSITION attribute's min/max corners of one converted
/// mesh. `None` when the mesh has no positions.
fn mesh_min_max(mesh: &Mesh) -> Option<(Vec3, Vec3)> {
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return None;
    };
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for p in positions {
        let v = Vec3::from(*p);
        min = min.min(v);
        max = max.max(v);
    }
    Some((min, max))
}

/// Spawn the selected item's ground model under the root (the
/// same NIF load + convert + material path as the doll and the
/// drop spawner — one model, the def's `ground_model`). Returns
/// the combined bounds for framing, or `None` when the item
/// has no ground model, the file is missing/unparsable, or the
/// mesh has no geometry (the viewport stays empty — the card
/// still shows).
#[allow(clippy::too_many_arguments)]
pub fn spawn_item_model_parts(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    assets: &mut AssetProvider,
    root: Entity,
    def: &ItemDef,
) -> Option<ModelBounds> {
    let ground_model = def.ground_model.as_ref()?;
    let nif_path = format!("meshes\\{ground_model}");
    let Some(bytes) = assets.read_file(&nif_path) else {
        crate::startup_log::log(&format!("item model: mesh not found: {nif_path}"));
        return None;
    };
    let Ok(nif) = dovah_nif::NifFile::from_bytes(&bytes) else {
        crate::startup_log::log(&format!("item model: NIF parse failed: {nif_path}"));
        return None;
    };
    let mut material_cache: HashMap<Option<String>, Handle<StandardMaterial>> = HashMap::new();
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    let mut spawned = 0;
    for sub in nif.meshes() {
        let Some(mesh) = convert::nif_mesh_to_bevy(&sub) else {
            continue;
        };
        if let Some((sub_min, sub_max)) = mesh_min_max(&mesh) {
            min = min.min(sub_min);
            max = max.max(sub_max);
        }
        let material = crate::player_model::doll_material(
            materials,
            images,
            assets,
            &mut material_cache,
            &sub.textures.diffuse,
        );
        commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            RenderLayers::layer(2),
            ChildOf(root),
            ItemModelEntity,
            ItemModelPart,
        ));
        spawned += 1;
    }
    if spawned == 0 {
        crate::startup_log::log(&format!("item model: {} has no geometry", def.name));
        return None;
    }
    Some(bounds_from_min_max(min, max))
}

/// Spawn the rig + the current selection's model when the
/// inventory opens (the rig does not exist yet). Runs every
/// frame; the rig query makes it a no-op once the rig exists.
/// The screenshot seed opens the UI in Startup — this runs on
/// the first Update frame, well before the capture.
#[allow(clippy::too_many_arguments)]
pub fn spawn_item_model_on_open(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut assets: Option<ResMut<AssetProvider>>,
    catalog: Res<ItemCatalog>,
    open: Res<InventoryOpen>,
    selected: Res<SelectedItem>,
    mut modeled: ResMut<ItemModelSelection>,
    mut yaw: ResMut<ItemModelYaw>,
    item_target: Option<Res<ItemModelTarget>>,
    rig: Query<Entity, With<ItemModelEntity>>,
) {
    if !open.0 || !rig.is_empty() {
        return;
    }
    let (Some(target), Some(assets)) = (item_target.as_deref(), assets.as_deref_mut()) else {
        return;
    };
    let (root, camera, light) = spawn_item_model_rig(&mut commands, target);
    modeled.0 = selected.0;
    yaw.0 = DEFAULT_YAW;
    let Some(def) = selected.0.and_then(|b| catalog.def(b)) else {
        return;
    };
    let Some(bounds) = spawn_item_model_parts(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        assets,
        root,
        def,
    ) else {
        return;
    };
    let (camera_transform, light_transform, intensity) = frame_transforms(&bounds);
    commands.entity(camera).insert(camera_transform);
    commands
        .entity(light)
        .insert((light_transform, item_model_light(intensity)));
}

/// Rebuild the model when the selection moved to a different
/// base: despawn the old parts, spawn the new item's, reframe
/// the camera + light, reset the yaw. No-op when the rig does
/// not exist yet (the on-open system owns that frame) or the
/// modeled base already matches the selection.
#[allow(clippy::too_many_arguments)]
pub fn rebuild_item_model_on_selection(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut assets: Option<ResMut<AssetProvider>>,
    catalog: Res<ItemCatalog>,
    open: Res<InventoryOpen>,
    selected: Res<SelectedItem>,
    mut modeled: ResMut<ItemModelSelection>,
    mut yaw: ResMut<ItemModelYaw>,
    roots: Query<Entity, With<ItemModelRoot>>,
    mut cameras: Query<&mut Transform, With<ItemModelCamera>>,
    light_entities: Query<Entity, With<ItemModelLight>>,
    mut lights: Query<&mut PointLight, With<ItemModelLight>>,
    parts: Query<Entity, With<ItemModelPart>>,
) {
    if !open.0 || selected.0 == modeled.0 {
        return;
    }
    let Ok(root) = roots.single() else {
        return;
    };
    for entity in parts.iter() {
        commands.entity(entity).despawn();
    }
    modeled.0 = selected.0;
    yaw.0 = DEFAULT_YAW;
    let Some(def) = selected.0.and_then(|b| catalog.def(b)) else {
        return;
    };
    let Some(assets) = assets.as_deref_mut() else {
        return;
    };
    let Some(bounds) = spawn_item_model_parts(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        assets,
        root,
        def,
    ) else {
        return;
    };
    let (camera_transform, light_transform, intensity) = frame_transforms(&bounds);
    if let Ok(mut transform) = cameras.single_mut() {
        *transform = camera_transform;
    }
    // The light's transform goes through Commands (two &mut
    // Transform queries in one system is a B0001 conflict);
    // its intensity through the PointLight query (a different
    // component, no conflict). The deferred transform lands at
    // the stage flush — one frame, invisible.
    if let Ok(light_entity) = light_entities.single() {
        commands.entity(light_entity).insert(light_transform);
    }
    if let Ok(mut light) = lights.single_mut() {
        light.intensity = intensity;
    }
}

/// Apply the yaw resource to the root's rotation. Runs every
/// frame (one assignment): gating on `is_changed()` would miss
/// the frame the root spawns on, leaving the model unturned.
pub fn apply_item_model_yaw(
    yaw: Res<ItemModelYaw>,
    mut roots: Query<&mut Transform, With<ItemModelRoot>>,
) {
    for mut transform in roots.iter_mut() {
        transform.rotation = Quat::from_rotation_y(yaw.0);
    }
}

/// Pixels of horizontal drag -> yaw radians: 0.01 rad/px, so a
/// 300 px drag turns the model nearly all the way around.
/// Pure — unit-tested.
pub fn drag_yaw(delta_x: f32) -> f32 {
    delta_x * 0.01
}

/// Drag the model viewport: rotate the model around Y (the
/// game's mouse rotation — horizontal drag only; the game does
/// not tilt the model vertically).
pub fn on_item_model_drag(event: On<PointerDrag>, mut yaw: ResMut<ItemModelYaw>) {
    yaw.0 += drag_yaw(event.delta.x);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_distance_fits_the_sphere_with_margin() {
        // A 0.5 m sphere at 40° FOV: 0.5 / tan(20°) x 1.15.
        let d = frame_distance(0.5, ITEM_MODEL_FOV);
        assert!((d - 1.58).abs() < 0.01, "distance {d}");
        // Twice the radius, twice the distance.
        assert!((frame_distance(1.0, ITEM_MODEL_FOV) - 2.0 * d).abs() < 0.001);
        // A degenerate model never puts the camera at the center.
        assert!(frame_distance(0.0, ITEM_MODEL_FOV) > 0.1);
    }

    #[test]
    fn frame_transforms_look_at_the_center_and_scale_the_light() {
        let bounds = ModelBounds {
            center: Vec3::new(0.0, 0.5, 0.0),
            radius: 0.5,
        };
        let (camera, light, intensity) = frame_transforms(&bounds);
        // The camera is on -Z of the center, looking at it.
        assert!(camera.translation.z < bounds.center.z);
        let forward = camera.forward();
        let to_center = (bounds.center - camera.translation).normalize();
        assert!(
            forward.dot(to_center) > 0.999,
            "camera does not look at the center"
        );
        // The light is up-right of the center, on the camera side.
        assert!(light.translation.y > bounds.center.y);
        assert!(light.translation.z < bounds.center.z);
        assert!(intensity > 0.0);
        // The invariant is irradiance, not intensity: intensity x
        // falloff at the light's distance is the 23,000 lux
        // target at any model size (linear law — the old test
        // pinned the inverse-square 4x ratio, which is gone).
        let d = (light.translation - bounds.center).length();
        let irradiance = intensity * crate::scene::skyrim_falloff(d, ITEM_MODEL_LIGHT_RANGE, 1.0);
        assert!(
            (irradiance - ITEM_MODEL_TARGET_LUX).abs() < 1.0,
            "irradiance {irradiance}"
        );
        // Twice the radius: the light sits ~2x farther, so the
        // intensity rises (falloff halves) — but only ~1.5-2x,
        // not the inverse-square 4x — and the irradiance at the
        // item is the same 23,000 lux.
        let big = ModelBounds {
            center: Vec3::ZERO,
            radius: 1.0,
        };
        let (_, big_light, big_intensity) = frame_transforms(&big);
        assert!(big_intensity > intensity);
        assert!(big_intensity / intensity < 2.5);
        let big_d = (big_light.translation - big.center).length();
        let big_irradiance =
            big_intensity * crate::scene::skyrim_falloff(big_d, ITEM_MODEL_LIGHT_RANGE, 1.0);
        assert!(
            (big_irradiance - ITEM_MODEL_TARGET_LUX).abs() < 1.0,
            "big irradiance {big_irradiance}"
        );
    }

    #[test]
    fn bounds_from_min_max_centers_and_spans() {
        let bounds = bounds_from_min_max(Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 2.0, 0.0));
        assert_eq!(bounds.center, Vec3::new(1.0, 1.0, 0.0));
        assert!((bounds.radius - std::f32::consts::SQRT_2).abs() < 0.001);
    }

    #[test]
    fn drag_yaw_scales_with_pixels() {
        assert_eq!(drag_yaw(100.0), 1.0);
        assert_eq!(drag_yaw(-50.0), -0.5);
        assert_eq!(drag_yaw(0.0), 0.0);
    }
}
