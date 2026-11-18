//! Player model (M17): the visible player character. It was
//! shown as a paper doll in the inventory screen until the
//! vanilla layout dropped the doll (vanilla has none); the
//! renderer stays intact here for the character/level-up
//! screens.
//!
//! The model reuses the M12 NPC pipeline — the same naked body
//! (NakedTorso/Hands/Feet for the player's race/sex, resolved by
//! `npc::body_parts`) and the same ARMO -> ARMA -> worn-model
//! chain for equipped armor (`npc::armor_model`, resolved at
//! catalog build). The head is the generic `MaleHead.nif` /
//! `FemaleHead.nif`: the player has no FaceGen (00000007.nif does
//! not exist in the Meshes BSAs — the real game's head is created
//! at character creation, which does not exist yet). A bald head
//! is the honest stand-in; hair comes with character creation.
//!
//! Rendering: the doll lives on render layer 1, lit by its own
//! warm point light and seen only by its own camera, which
//! renders to a small image. The main camera (layer 0) never
//! sees the doll, and the doll camera never sees the world — so
//! the model stands in a void with a dark backdrop, not floating
//! in the inn. The shield does not display yet (it needs a hand
//! attachment — no skeleton work in M17), and neither does the
//! equipped weapon.

use crate::assets::AssetProvider;
use crate::convert;
use crate::inventory::{EquipSlot, Inventory, ItemCatalog};
use bevy::asset::{Assets, Handle};
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, RenderTarget};
use bevy::color::Color;
use bevy::ecs::prelude::*;
use bevy::image::Image;
use bevy::light::PointLight;
use bevy::math::Vec3;
use bevy::mesh::Mesh;
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::render::mesh::Mesh3d;
use bevy::render::render_resource::TextureFormat;
use bevy::transform::components::Transform;
use std::collections::HashMap;

/// The paper-doll portrait size (2:3, like the UI panel).
pub const DOLL_IMAGE_WIDTH: u32 = 512;
pub const DOLL_IMAGE_HEIGHT: u32 = 768;

/// The render-target image the doll camera draws into. The
/// vanilla inventory does not show the doll (no character doll
/// in the game's inventory), so nothing reads the image yet —
/// the character/level-up screens will. The renderer stays
/// intact for them; the field is allow(dead_code) until then.
#[derive(Resource, Clone)]
pub struct DollTarget {
    #[allow(dead_code)]
    pub image: Handle<Image>,
}

/// Marker for every doll entity (camera, light, model parts):
/// despawned when the inventory closes.
#[derive(Component)]
pub struct DollEntity;

/// Marker for the doll's model parts only: despawned on an
/// equipment rebuild (the camera and light stay).
#[derive(Component)]
pub struct DollPart;

/// Create the doll render target (Startup, from the inventory
/// plugin — it must exist before the inventory can open).
pub fn create_doll_target(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut target = Image::new_target_texture(
        DOLL_IMAGE_WIDTH,
        DOLL_IMAGE_HEIGHT,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    target.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
    let handle = images.add(target);
    commands.insert_resource(DollTarget { image: handle });
}

/// The model paths that draw the player: bare body, head, and the
/// worn model of each equipped armor piece (shield excluded — no
/// hand attachment in M17). Paths are relative to `meshes/`, the
/// `NpcModels` convention. Pure — unit-tested.
pub fn doll_model_paths(catalog: &ItemCatalog, inventory: &Inventory) -> Vec<String> {
    let mut parts: Vec<String> = catalog.player_body.clone();
    if !catalog.player_head.is_empty() {
        parts.push(catalog.player_head.clone());
    }
    // Fixed slot order — the equipment map is a HashMap, so
    // iterating it would order the parts nondeterministically.
    // Shield is skipped (no hand attachment in M17).
    for slot in [
        EquipSlot::Head,
        EquipSlot::Body,
        EquipSlot::Hands,
        EquipSlot::Feet,
    ] {
        let Some(base_form_id) = inventory.equipped_armor.get(&slot) else {
            continue;
        };
        if let Some(def) = catalog.def(*base_form_id) {
            if let Some(model) = &def.worn_model {
                parts.push(model.clone());
            }
        }
    }
    parts
}

/// Spawn the whole doll: camera + light + model parts. Unused
/// since the vanilla inventory dropped the doll — kept intact
/// for the character/level-up screens (see the module doc).
#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
pub fn spawn_doll(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    assets: &mut AssetProvider,
    target: &DollTarget,
    catalog: &ItemCatalog,
    inventory: &Inventory,
) {
    spawn_doll_rig_with_target(commands, target);
    spawn_doll_parts(
        commands, meshes, materials, images, assets, catalog, inventory,
    );
}

/// Spawn the doll camera + light with an explicit target handle.
/// Split from `spawn_doll` so the rig can be built by systems
/// that hold the target resource. Unused since the vanilla
/// inventory dropped the doll — kept for the character screens.
#[allow(dead_code)]
pub fn spawn_doll_rig_with_target(commands: &mut Commands, target: &DollTarget) {
    // The model faces Bevy -Z (Skyrim +Y is forward), so the
    // camera sits on -Z looking back at it: feet at y=0, head at
    // ~1.75 m, FOV 40° at 2.8 m frames the whole character.
    let camera_transform = Transform::from_translation(Vec3::new(0.0, 1.0, -2.8))
        .looking_at(Vec3::new(0.0, 0.95, 0.0), Vec3::Y);
    commands.spawn((
        bevy::camera::Camera3d::default(),
        bevy::camera::Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.05, 0.05, 0.08)),
            ..Default::default()
        },
        RenderTarget::Image(target.image.clone().into()),
        RenderLayers::layer(1),
        bevy::camera::Projection::Perspective(bevy::camera::PerspectiveProjection {
            fov: 40f32.to_radians(),
            ..Default::default()
        }),
        camera_transform,
        DollEntity,
    ));
    // Warm key light, front-right above — the inn-firelight feel.
    // Intensity 37,000 under the linear falloff law: the old
    // 150,000 put ~21,400 lux on the doll's centre (2.56 m away,
    // I/d^2 x window), and 37,000 x (1 - 2.56/6) is the same
    // 21,400 lux. The old 150,000 under linear falloff would
    // have been 86,000 lux — a blown-out doll.
    commands.spawn((
        PointLight {
            intensity: 37_000.0,
            range: 6.0,
            color: Color::srgb(1.0, 0.85, 0.65),
            ..Default::default()
        },
        Transform::from_translation(Vec3::new(1.2, 2.0, -2.0)),
        RenderLayers::layer(1),
        DollEntity,
    ));
}

/// Spawn the doll's model parts (body + head + equipped armor).
/// Called on open (via `spawn_doll`) and on equipment change
/// (after the old parts are despawned).
#[allow(clippy::too_many_arguments)]
pub fn spawn_doll_parts(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    assets: &mut AssetProvider,
    catalog: &ItemCatalog,
    inventory: &Inventory,
) {
    let paths = doll_model_paths(catalog, inventory);
    let mut material_cache: HashMap<Option<String>, Handle<StandardMaterial>> = HashMap::new();
    let mut spawned = 0;
    for path in &paths {
        let nif_path = format!("meshes\\{path}");
        let Some(bytes) = assets.read_file(&nif_path) else {
            crate::startup_log::log(&format!("doll: mesh not found: {nif_path}"));
            continue;
        };
        let Ok(nif) = dovah_nif::NifFile::from_bytes(&bytes) else {
            crate::startup_log::log(&format!("doll: NIF parse failed: {nif_path}"));
            continue;
        };
        for sub in nif.meshes() {
            let Some(mesh) = convert::nif_mesh_to_bevy(&sub) else {
                continue;
            };
            let material = doll_material(
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
                RenderLayers::layer(1),
                DollEntity,
                DollPart,
            ));
            spawned += 1;
        }
    }
    crate::startup_log::log(&format!(
        "doll: {spawned} sub-meshes from {} parts",
        paths.len()
    ));
}

/// Material for a doll sub-mesh: textured when the diffuse
/// resolves, plain light grey otherwise. Opaque — body and armor
/// NIFs carry no alpha-tested parts in practice (the A2C foliage
/// path lives in scene.rs and is not needed here). Shared with
/// the inventory's drop spawner (a dropped item's ground model
/// takes the same material path).
pub(crate) fn doll_material(
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    assets: &mut AssetProvider,
    cache: &mut HashMap<Option<String>, Handle<StandardMaterial>>,
    diffuse: &Option<String>,
) -> Handle<StandardMaterial> {
    if let Some(handle) = cache.get(diffuse) {
        return handle.clone();
    }
    let mut material = StandardMaterial::default();
    let mut textured = false;
    if let Some(path) = diffuse {
        if let Some(bytes) = assets.read_file(path) {
            if let Some(image) = convert::dds_to_bevy_image(&bytes) {
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
    let handle = materials.add(material);
    cache.insert(diffuse.clone(), handle.clone());
    handle
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{ItemDef, ItemKind};

    fn armor_def(base: u32, worn_model: Option<&str>) -> ItemDef {
        ItemDef {
            base_form_id: base,
            name: format!("Armor {base:#x}"),
            kind: ItemKind::Armor,
            value: 10,
            weight: 5.0,
            damage: 0,
            speed: 1.0,
            reach: 1.0,
            is_bow: false,
            projectile_model: None,
            projectile_speed: 0.0,
            projectile_gravity: 0.0,
            armor_rating: 10.0,
            slot: None,
            worn_model: worn_model.map(|s| s.to_string()),
            description: None,
            effects: Vec::new(),
            ground_model: None,
        }
    }

    #[test]
    fn doll_paths_are_body_head_and_worn_armor() {
        let mut catalog = ItemCatalog {
            player_body: vec!["Body.nif".to_string(), "Hands.nif".to_string()],
            player_head: "MaleHead.nif".to_string(),
            ..Default::default()
        };
        catalog
            .defs
            .insert(0x200, armor_def(0x200, Some("Armor\\Cuirass.nif")));
        catalog
            .defs
            .insert(0x201, armor_def(0x201, Some("Armor\\Helmet.nif")));
        catalog
            .defs
            .insert(0x202, armor_def(0x202, Some("Armor\\Shield.nif")));
        catalog.defs.insert(0x203, armor_def(0x203, None)); // no worn model: skipped
        let mut inventory = Inventory::default();
        inventory.equipped_armor.insert(EquipSlot::Body, 0x200);
        inventory.equipped_armor.insert(EquipSlot::Head, 0x201);
        inventory.equipped_armor.insert(EquipSlot::Shield, 0x202); // skipped: no hand
        inventory.equipped_armor.insert(EquipSlot::Hands, 0x203); // skipped: no model
        let paths = doll_model_paths(&catalog, &inventory);
        // Fixed slot order: head before body, regardless of
        // insertion order into the equipment map.
        assert_eq!(
            paths,
            vec![
                "Body.nif",
                "Hands.nif",
                "MaleHead.nif",
                "Armor\\Helmet.nif",
                "Armor\\Cuirass.nif",
            ]
        );
    }

    #[test]
    fn doll_without_equipment_is_body_and_head() {
        let catalog = ItemCatalog {
            player_body: vec!["Body.nif".to_string()],
            player_head: "MaleHead.nif".to_string(),
            ..Default::default()
        };
        let inventory = Inventory::default();
        assert_eq!(
            doll_model_paths(&catalog, &inventory),
            vec!["Body.nif", "MaleHead.nif"]
        );
    }
}
