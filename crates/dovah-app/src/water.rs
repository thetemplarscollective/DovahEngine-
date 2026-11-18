//! River water: one flat plane per exterior cell, at the
//! cell's resolved water level, animated by the WATR flow
//! vector.
//!
//! Data model (see `dovah_esm::water`): a CELL's XCLW is its
//! water height, XCWT its water type, falling back to the
//! worldspace DNAM/NAM2 defaults; the WATR DNAM carries the
//! colours + fresnel, and NAM0 is the flow vector (direction
//! x speed, Skyrim space). `cell::load_exterior` resolves all
//! of it into `CellData.waters` — only cells whose terrain
//! dips below the level get a surface.
//!
//! Rendering (the Mudcrab shape, with our own code): a flat
//! quad spanning the whole cell at the water height, a
//! StandardMaterial in the transparent pass — deep colour at
//! 60% alpha so the riverbed shows through, metallic 0.15 /
//! roughness 0.06 / reflectance 0.9 for sun glints, the
//! WATR normal map (DefaultWater.dds) for ripple detail, and
//! a subtle procedural noise texture so the flow stays
//! visible even with `--no-normal-maps` (desktop proofs skip
//! normal maps). Reflection is NOT rendered: there is no LOD
//! world to reflect yet, and reflecting the full-detail world
//! would double the draw cost — the fresnel fade to the WATR
//! reflection colour is the no-reflection fallback, and needs
//! a custom shader, so it waits for the reflection milestone
//! (see the handoff). No underwater rendering either: the
//! plane is single-sided, invisible from below, so walking
//! into the riverbed reads as wading, not as a black ceiling.
//!
//! Flow: [`animate_water_flow`] scrolls each plane's UVs by
//! the NAM0 vector (converted to UV space: +u east, +v
//! north) at `speed / tile_size` tiles per second. The mesh
//! is built with MAIN_WORLD usage so the UV attribute stays
//! mutable — RENDER_WORLD meshes drop their CPU copy after
//! upload and cannot be scrolled.

use bevy::asset::RenderAssetUsages;
use bevy::ecs::prelude::*;
use bevy::image::{Image, ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::math::{Vec2, Vec3};
use bevy::mesh::{Indices, Mesh};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::time::Time;
use dovah_esm::water::WaterType;

/// Normal-map / noise tiles across one cell (4096 Skyrim
/// units = 58.5 m, so one tile is ~7.3 m — the 2048-px water
/// normal map still resolves ~280 px/m).
pub const WATER_TILE_COUNT: f32 = 8.0;

/// Water opacity: 60% cover (Mudcrab's measured value for
/// DefaultWater — the riverbed shows without depth fog).
pub const WATER_ALPHA: f32 = 0.6;

/// The NAM0 flow magnitude is read as metres per second: the
/// White River's 2.88 m/s is a fast mountain river, and the
/// scroll it produces (0.39 tiles/s) is visibly flowing
/// without strobing. The units in the file are not documented
/// anywhere — if a later measurement proves them to be
/// Skyrim units/s (4 cm/s, implausibly still), this constant
/// is the one place to fix.
pub const FLOW_SPEED_UNIT: f32 = 1.0;

/// One spawned water plane: its flow (UV space) and its base
/// UVs. The system rewrites the mesh UVs as base + offset
/// every frame; the offset wraps at 1.0 (the textures repeat,
/// so wrapping is invisible).
#[derive(Component, Debug, Clone)]
pub struct WaterFlow {
    /// UV scroll direction (unit vector: +u east, +v north).
    pub direction: Vec2,
    /// UV tiles scrolled per second.
    pub speed: f32,
    /// Accumulated UV offset, kept in [0, 1).
    pub offset: Vec2,
    /// The plane's un-scrolled UVs (4 verts).
    pub base_uvs: [[f32; 2]; 4],
}

impl WaterFlow {
    /// Build the component from a WATR flow vector (Skyrim
    /// space, [x, y] = [east, north]). The UV map has +u east
    /// and +v north, so the Skyrim vector maps straight onto
    /// UV space — no axis flip. A zero vector is still water
    /// (speed 0, direction arbitrary but unit).
    pub fn from_watr_flow(flow: [f32; 2], base_uvs: [[f32; 2]; 4]) -> Self {
        let v = Vec2::new(flow[0], flow[1]);
        let len = v.length();
        let (direction, speed) = if len > 1e-6 {
            let tile_m =
                crate::land::CELL_SPAN / WATER_TILE_COUNT / crate::coords::SKYRIM_UNITS_PER_METER;
            (v / len, len * FLOW_SPEED_UNIT / tile_m)
        } else {
            (Vec2::X, 0.0)
        };
        WaterFlow {
            direction,
            speed,
            offset: Vec2::ZERO,
            base_uvs,
        }
    }
}

/// Pure UV-scroll step: the offset delta for one frame. Split
/// out of the system so the math is unit-testable without a
/// Bevy app.
pub fn flow_uv_delta(direction: Vec2, speed: f32, dt: f32) -> Vec2 {
    direction * speed * dt
}

/// Scroll every water plane's UVs by its flow. Runs in
/// InGame on both app paths (registered in PlayerPlugin for
/// the windowed path and ScreenshotPlugin for the screenshot
/// path — lib.rs owns neither registration, so the two
/// plugins each carry one line). `Time` is optional: the
/// headless unit-test app has no TimePlugin, and the system
/// must no-op there, not panic.
pub fn animate_water_flow(
    time: Option<Res<Time>>,
    mut meshes: ResMut<bevy::asset::Assets<Mesh>>,
    mut query: Query<(&mut WaterFlow, &bevy::render::mesh::Mesh3d)>,
) {
    let Some(time) = time else {
        return;
    };
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (mut flow, mesh_handle) in query.iter_mut() {
        if flow.speed <= 0.0 {
            continue;
        }
        let (direction, speed) = (flow.direction, flow.speed);
        flow.offset += flow_uv_delta(direction, speed, dt);
        // Wrap at 1.0: the textures repeat, so the wrap is
        // invisible, and it keeps the floats small forever.
        flow.offset = Vec2::new(flow.offset.x.fract(), flow.offset.y.fract());
        let Some(mut mesh) = meshes.get_mut(mesh_handle.id()) else {
            continue;
        };
        let uvs: Vec<[f32; 2]> = flow
            .base_uvs
            .iter()
            .map(|[u, v]| [u + flow.offset.x, v + flow.offset.y])
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    }
}

/// Build a water plane mesh for one cell: a flat quad spanning
/// the whole cell at `level` (Skyrim units), in Bevy world
/// space. UVs run 0..[`WATER_TILE_COUNT`] with +u east and +v
/// north; normals point up; tangents are (+X, w=+1), matching
/// the UV layout (T x B = N: X x -Z = +Y). MAIN_WORLD usage:
/// the flow system rewrites the UV attribute every frame.
///
/// Returns the mesh and its base UVs (for the [`WaterFlow`]
/// component).
pub fn water_plane_mesh(grid: (i32, i32), level: f32) -> (Mesh, [[f32; 2]; 4]) {
    let (gx, gy) = grid;
    let span = crate::land::CELL_SPAN / crate::coords::SKYRIM_UNITS_PER_METER;
    let min_x = gx as f32 * span;
    let max_x = (gx + 1) as f32 * span;
    // Bevy z = -Skyrim y: the cell's north edge (gy+1) is the
    // SMALLER z.
    let min_z = -((gy + 1) as f32 * span);
    let max_z = -(gy as f32 * span);
    let y = level / crate::coords::SKYRIM_UNITS_PER_METER;
    let t = WATER_TILE_COUNT;
    // SW, SE, NW, NE — the same vertex order as
    // `LandData::terrain_mesh`, so the same (a,b,c),(b,d,c)
    // winding is upward-facing here too.
    let positions = [
        [min_x, y, max_z],
        [max_x, y, max_z],
        [min_x, y, min_z],
        [max_x, y, min_z],
    ];
    let uvs = [[0.0, 0.0], [t, 0.0], [0.0, t], [t, t]];
    let mut mesh = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        positions.iter().map(|p| Vec3::from(*p)).collect::<Vec<_>>(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![Vec3::Y; 4]);
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        uvs.iter()
            .map(|[u, v]| Vec2::new(*u, *v))
            .collect::<Vec<_>>(),
    );
    // Tangent +X with handedness +1: bitangent is
    // cross(N, T) * w = (Y x X) * 1 = -Z, i.e. increasing v
    // runs north, matching the UV layout.
    mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, vec![[1.0, 0.0, 0.0, 1.0]; 4]);
    mesh.insert_indices(Indices::U32(vec![0, 1, 2, 1, 3, 2]));
    (mesh, uvs)
}

/// The water's base colour: the WATR deep colour at 60%
/// alpha (Mudcrab's measured cover for DefaultWater). The
/// shallow colour is not used yet — without depth
/// information there is nothing to blend it against; the
/// depth-based shallow/deep blend waits for the water-shader
/// milestone.
pub fn water_base_color(watr: &WaterType) -> bevy::color::Color {
    let [r, g, b] = watr.deep;
    bevy::color::Color::srgba(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        WATER_ALPHA,
    )
}

/// A subtle procedural noise texture for the water's base
/// colour: two octaves of deterministic value noise in
/// 0.85..1.0 grey, so the scrolling UVs read as moving
/// mottling on the surface even when the normal map is
/// skipped (`--no-normal-maps`). The texture multiplies the
/// deep colour — near-white, so it tints nothing, it only
/// varies the brightness. Repeat sampling: the UVs run past
/// 1.0 by design.
pub fn water_noise_texture() -> Image {
    const SIZE: u32 = 128;
    let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    // Deterministic hash -> [0, 1): splitmix-style, no RNG
    // dependency, same texture every run.
    fn hash(x: i32, y: i32) -> f32 {
        let mut h = (x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263)) as u32;
        h = (h ^ (h >> 13)).wrapping_mul(1_274_126_579);
        h = h ^ (h >> 16);
        (h % 10_000) as f32 / 10_000.0
    }
    fn noise(x: f32, y: f32, period: f32) -> f32 {
        let fx = x / period;
        let fy = y / period;
        let x0 = fx.floor() as i32;
        let y0 = fy.floor() as i32;
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        // Smoothstep interpolation between the lattice points.
        let sx = tx * tx * (3.0 - 2.0 * tx);
        let sy = ty * ty * (3.0 - 2.0 * ty);
        let h00 = hash(x0, y0);
        let h10 = hash(x0 + 1, y0);
        let h01 = hash(x0, y0 + 1);
        let h11 = hash(x0 + 1, y0 + 1);
        (h00 * (1.0 - sx) + h10 * sx) * (1.0 - sy) + (h01 * (1.0 - sx) + h11 * sx) * sy
    }
    for y in 0..SIZE {
        for x in 0..SIZE {
            // Two octaves: broad swells + fine ripple.
            let n = 0.65 * noise(x as f32, y as f32, 23.0)
                + 0.35 * noise(x as f32 + 61.0, y as f32 + 37.0, 7.0);
            let v = (0.85 + 0.15 * n).clamp(0.0, 1.0);
            let byte = (v * 255.0) as u8;
            data.extend_from_slice(&[byte, byte, byte, 255]);
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        ..Default::default()
    });
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::render::mesh::VertexAttributeValues;

    fn river_watr() -> WaterType {
        WaterType {
            form_id: 0x000e717c,
            reflectivity: 0.8,
            fresnel: 0.1,
            shallow: [38, 39, 24],
            deep: [5, 14, 18],
            reflection: [119, 140, 157],
            flow: [2.54, 1.35],
            normal_map: Some("Data\\Textures\\Water\\DefaultWater.dds".to_string()),
        }
    }

    #[test]
    fn plane_spans_the_cell_at_the_water_level() {
        // Riverwood centre cell (5,-11), water level -250
        // Skyrim units = -3.571 m. Cell span 4096 u = 58.51 m.
        let (mesh, uvs) = water_plane_mesh((5, -11), -250.0);
        let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() {
            VertexAttributeValues::Float32x3(p) => p.clone(),
            _ => panic!("positions are Float32x3"),
        };
        assert_eq!(positions.len(), 4);
        let span = 4096.0 / 70.0;
        // x: 5*span .. 6*span; z: north edge (gy+1) is min z.
        assert!((positions[0][0] - 5.0 * span).abs() < 1e-4); // SW x
        assert!((positions[1][0] - 6.0 * span).abs() < 1e-4); // SE x
        assert!((positions[0][2] - 11.0 * span).abs() < 1e-4); // SW z (south edge)
        assert!((positions[2][2] - 10.0 * span).abs() < 1e-4); // NW z (north edge)
        for p in &positions {
            assert!((p[1] - (-250.0 / 70.0)).abs() < 1e-5, "y = {p:?}");
        }
        // UVs: 0..TILE, +u east, +v north.
        assert_eq!(uvs[0], [0.0, 0.0]);
        assert_eq!(uvs[1], [WATER_TILE_COUNT, 0.0]);
        assert_eq!(uvs[2], [0.0, WATER_TILE_COUNT]);
        assert_eq!(uvs[3], [WATER_TILE_COUNT, WATER_TILE_COUNT]);
        // Upward-facing winding: (b-a) x (c-a) has +Y.
        let a = Vec3::from(positions[0]);
        let b = Vec3::from(positions[1]);
        let c = Vec3::from(positions[2]);
        assert!((b - a).cross(c - a).y > 0.0);
        // Tangents: +X, w=+1 (bitangent -Z = north).
        match mesh.attribute(Mesh::ATTRIBUTE_TANGENT).unwrap() {
            VertexAttributeValues::Float32x4(t) => {
                assert_eq!(t, &vec![[1.0, 0.0, 0.0, 1.0]; 4]);
            }
            _ => panic!("tangents are Float32x4"),
        }
    }

    #[test]
    fn flow_direction_maps_skyrim_to_uv_without_a_flip() {
        // NE flow (2.54, 1.35): +u (east) and +v (north).
        // A flipped mapping would send the river SW.
        let (_, uvs) = water_plane_mesh((5, -11), -250.0);
        let flow = WaterFlow::from_watr_flow([2.54, 1.35], uvs);
        assert!(flow.direction.x > 0.0 && flow.direction.y > 0.0);
        assert!((flow.direction.length() - 1.0).abs() < 1e-6);
        // Speed: 2.876 m/s over a 7.314 m tile = 0.393 tiles/s.
        assert!((flow.speed - 0.393).abs() < 0.005, "speed {}", flow.speed);
        // Still water: speed 0.
        let still = WaterFlow::from_watr_flow([0.0, 0.0], uvs);
        assert_eq!(still.speed, 0.0);
    }

    #[test]
    fn flow_uv_delta_scales_with_time() {
        let d = flow_uv_delta(Vec2::new(0.883, 0.469), 0.393, 1.0 / 30.0);
        assert!((d.x - 0.0116).abs() < 0.001);
        assert!((d.y - 0.0061).abs() < 0.001);
        assert_eq!(flow_uv_delta(Vec2::X, 1.0, 0.0), Vec2::ZERO);
    }

    #[test]
    fn base_color_is_deep_at_sixty_percent() {
        let c = water_base_color(&river_watr());
        let bevy::color::Color::Srgba(s) = c else {
            panic!("expected Srgba, got {c:?}");
        };
        assert!((s.red - 5.0 / 255.0).abs() < 1e-6);
        assert!((s.green - 14.0 / 255.0).abs() < 1e-6);
        assert!((s.blue - 18.0 / 255.0).abs() < 1e-6);
        assert!((s.alpha - 0.6).abs() < 1e-6);
    }

    #[test]
    fn flow_system_scrolls_mesh_uvs_in_app() {
        // The render proof (bridge f10 vs f60) shows the flow
        // as a ~1 LSB structured diff — small because the
        // water colour is near-black. This test locks the
        // mechanism deterministically: 100 ms of flow at
        // 0.25 tiles/s must move every UV by +0.025 u. (100 ms
        // stays under Bevy's 250 ms virtual-time max-delta
        // clamp; a 1 s step would be clamped to 0.25 s.)
        use bevy::app::App;
        use bevy::asset::Assets;
        use bevy::prelude::MinimalPlugins;
        use bevy::render::mesh::Mesh3d;
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // MinimalPlugins has no MeshPlugin, so the mesh store
        // is inserted directly (init_asset needs the
        // AssetServer, which only exists after app.finish()).
        app.insert_resource(Assets::<Mesh>::default());
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            100,
        )));
        app.add_systems(bevy::app::Update, animate_water_flow);
        let (mesh, base_uvs) = water_plane_mesh((5, -11), -250.0);
        let handle = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
        app.world_mut().spawn((
            WaterFlow {
                direction: Vec2::X,
                speed: 0.25,
                offset: Vec2::ZERO,
                base_uvs,
            },
            Mesh3d(handle.clone()),
        ));
        app.update();
        // Time reports delta 0 on the FIRST update (no
        // previous instant — bevy_time docs) — the second
        // update is the one that advances by 1.0 s.
        app.update();
        let after = app
            .world()
            .resource::<Assets<Mesh>>()
            .get(&handle)
            .expect("water mesh still present");
        let VertexAttributeValues::Float32x2(uvs) = after
            .attribute(Mesh::ATTRIBUTE_UV_0)
            .expect("uv attribute present")
        else {
            panic!("uvs are not float32x2");
        };
        assert_eq!(uvs.len(), 4);
        for (i, [u, v]) in uvs.iter().enumerate() {
            assert!(
                (u - (base_uvs[i][0] + 0.025)).abs() < 1e-6,
                "uv {i} u = {u}, expected {}",
                base_uvs[i][0] + 0.025
            );
            assert!(
                (v - base_uvs[i][1]).abs() < 1e-6,
                "uv {i} v = {v}, expected {}",
                base_uvs[i][1]
            );
        }
    }

    #[test]
    fn noise_texture_is_subtle_near_white_and_repeating() {
        let image = water_noise_texture();
        assert_eq!(image.width(), 128);
        assert_eq!(image.height(), 128);
        let data = image.data.as_ref().unwrap();
        let mut min = 255u8;
        let mut max = 0u8;
        let (chunks, _) = data.as_chunks::<4>();
        for px in chunks {
            min = min.min(px[0]);
            max = max.max(px[0]);
            assert_eq!(px[3], 255, "opaque alpha");
            assert_eq!(px[0], px[1]);
            assert_eq!(px[1], px[2]);
        }
        // 0.85..1.0 grey: subtle mottling, never dark.
        assert!(min >= 210, "min {min}");
        assert!(max > 240, "max {max}");
        assert!(max - min > 10, "must actually vary");
        let ImageSampler::Descriptor(desc) = &image.sampler else {
            panic!("expected a descriptor sampler");
        };
        assert_eq!(desc.address_mode_u, ImageAddressMode::Repeat);
        assert_eq!(desc.address_mode_v, ImageAddressMode::Repeat);
    }
}
