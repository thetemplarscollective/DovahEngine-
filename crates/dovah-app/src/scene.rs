//! Scene building: turn a loaded [`CellData`] into Bevy entities.
//!
//! Each placed object becomes either a real mesh (its `.nif` found
//! in the data folder and parsed by dovah-nif, with its diffuse
//! texture applied when the `.dds` resolves) or a placeholder box
//! sized from the base record's OBND and colored by base type. The
//! swap from placeholder to real mesh is one code path —
//! [`spawn_object`] tries the real mesh first and falls back — so
//! when the Meshes BSA lands, the same scene builds with real
//! geometry and no rewrite.
//!
//! Placeholder boxes are double-sided: the room's own shell pieces
//! are placeholders too, and the camera stands *inside* them.

use crate::assets::AssetProvider;
use crate::cell::{CellData, PlacedObject};
use crate::collision::{
    collision_from_shapes, point_in_triangle_2d, transform_footprint, triangle_overlaps_cell_2d,
    CachedCollision, ObstacleGrid,
};
use crate::convert;
use crate::coords;
use crate::doors;
use crate::land::{LandData, CELL_SPAN};
use crate::mill;
use crate::sawmill;
use bevy::asset::{Assets, Handle};
use bevy::color::{Color, LinearRgba};
use bevy::ecs::prelude::*;
use bevy::image::Image;
use bevy::light::{DirectionalLight, PointLight};
use bevy::material::AlphaMode;
use bevy::math::Vec3;
use bevy::mesh::{Indices, Mesh, VertexAttributeValues};
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::NextState;
use bevy::render::mesh::Mesh3d;
use bevy::shape::{Capsule3d, Cuboid, Sphere};
use dovah_nif::AlphaBlend;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The room's extent in Bevy space, for the player clamp and the
/// screenshot camera.
#[derive(Debug, Clone, Copy, Resource)]
pub struct RoomBounds {
    pub min: Vec3,
    pub max: Vec3,
    /// Height of the main floor (median object height), metres.
    pub floor_y: f32,
}

impl RoomBounds {
    pub fn from_cell(cell: &CellData) -> Self {
        let (min_sky, max_sky) = cell.position_bounds();
        let (min, max) = coords::to_bevy_bounds(min_sky, max_sky);
        let median = cell.median_position();
        let floor_y = median[2] / coords::SKYRIM_UNITS_PER_METER;
        RoomBounds { min, max, floor_y }
    }

    /// Bounds for an exterior block: the exact 3x3 grid rectangle
    /// (not the object span — a far-flung tree must not extend the
    /// walkable area past the loaded terrain), with `floor_y` the
    /// terrain height at the block's centre.
    pub fn from_exterior(center_grid: (i32, i32), floor_y: f32) -> Self {
        let min_sky = [
            (center_grid.0 - 1) as f32 * CELL_SPAN,
            (center_grid.1 - 1) as f32 * CELL_SPAN,
            0.0,
        ];
        let max_sky = [
            (center_grid.0 + 2) as f32 * CELL_SPAN,
            (center_grid.1 + 2) as f32 * CELL_SPAN,
            0.0,
        ];
        let (min, max) = coords::to_bevy_bounds(min_sky, max_sky);
        // The vertical extent is generous outdoors (the terrain
        // rises and falls; the clamp only matters for falling out
        // of the world).
        let min = Vec3::new(min.x, -50.0, min.z);
        let max = Vec3::new(max.x, 200.0, max.z);
        RoomBounds { min, max, floor_y }
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) / 2.0
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }
}

/// Marker for every entity the scene build spawns (objects, lights,
/// terrain, sky, sun). Door transitions despawn all of them before
/// the next scene builds. The player/camera are NOT tagged — they
/// are despawned separately by their own markers.
#[derive(Component)]
pub struct SceneEntity;

/// Terrain heights for an exterior scene, for the player's ground
/// (M9): the floor grid covers mesh tops (buildings, bridges), the
/// terrain sample covers the ground itself. The player stands on
/// whichever is higher under their feet.
#[derive(Resource)]
pub struct TerrainHeights {
    lands: HashMap<(i32, i32), LandData>,
}

impl TerrainHeights {
    pub fn new(lands: &[LandData]) -> Self {
        TerrainHeights {
            lands: lands.iter().map(|l| (l.grid, l.clone())).collect(),
        }
    }

    /// Terrain height (metres, Bevy Y) at a Bevy-space (x, z).
    /// `None` outside the loaded blocks.
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        // Bevy -> Skyrim: x_sky = x * 70, y_sky = -z * 70.
        let sky_x = x * coords::SKYRIM_UNITS_PER_METER;
        let sky_y = -z * coords::SKYRIM_UNITS_PER_METER;
        let gx = (sky_x / CELL_SPAN).floor() as i32;
        let gy = (sky_y / CELL_SPAN).floor() as i32;
        let land = self.lands.get(&(gx, gy))?;
        let lx = sky_x - gx as f32 * CELL_SPAN;
        let ly = sky_y - gy as f32 * CELL_SPAN;
        land.sample_height(lx, ly)
            .map(|h| h / coords::SKYRIM_UNITS_PER_METER)
    }
}

/// What the scene build produced, for the report.
#[derive(Debug, Default, Resource)]
pub struct SceneSummary {
    pub objects_total: usize,
    pub real_mesh_objects: usize,
    pub real_submeshes: usize,
    pub placeholder_objects: usize,
    pub actors: usize,
    /// Actor model parts spawned with real meshes (M12): body,
    /// outfit, and head NIFs. An actor whose parts all failed to
    /// spawn falls back to a capsule and counts 0 parts.
    pub actor_parts: usize,
    pub lights: usize,
    pub textures_loaded: usize,
    pub textures_missing: usize,
    /// The missing diffuse paths (deduped, first 20) — the same
    /// diagnostic idea as `largest`: a missing texture is usually
    /// a garbage path in one NIF, and the list names it.
    pub missing_textures: Vec<String>,
    pub meshes_not_found: usize,
    pub mesh_parse_failed: usize,
    /// Refs whose model parsed but held no drawable geometry
    /// (v0.4.7: editor markers, particle-only NIFs) — they
    /// spawn nothing, not even a placeholder.
    pub mesh_no_drawable: usize,
    /// Unique NIFs whose Havok collision was used for the
    /// floor/obstacle grids (per-object collision milestone).
    pub collision_nifs: usize,
    /// Terrain cells spawned (exterior scenes, M9).
    pub terrain_cells: usize,
    /// Water surfaces spawned (exterior scenes, water
    /// milestone): one flat plane per cell whose terrain dips
    /// below its resolved water level.
    pub water_cells: usize,
    /// Placeholders skipped because their OBND box is huge (room
    /// shells and other hollow pieces — see `spawn_placeholder`).
    pub skipped_shells: usize,
    /// Largest placeholder boxes by volume: (volume m^3, edid, size
    /// m, position m). Kept for diagnostics — a giant box is usually
    /// a room shell or a bad OBND, and it is the first suspect when
    /// a viewpoint is occluded.
    pub largest: Vec<(f32, String, Vec3, Vec3)>,
}

impl SceneSummary {
    pub fn print(&self, cell: &CellData) {
        println!("--- scene summary: {} ---", cell.edid);
        println!("placed objects in cell: {}", self.objects_total);
        println!(
            "  real meshes:   {} objects ({} sub-meshes)",
            self.real_mesh_objects, self.real_submeshes
        );
        println!("  placeholders:  {} objects", self.placeholder_objects);
        println!(
            "  actors:        {} ({} model parts)",
            self.actors, self.actor_parts
        );
        println!("  lights:        {}", self.lights);
        if self.terrain_cells > 0 {
            println!("  terrain cells: {}", self.terrain_cells);
        }
        if self.water_cells > 0 {
            println!("  water cells:   {}", self.water_cells);
        }
        println!(
            "  textures:      {} loaded, {} missing",
            self.textures_loaded, self.textures_missing
        );
        for path in &self.missing_textures {
            println!("    missing:     {path}");
        }
        println!(
            "  mesh files:    {} not in data, {} failed to parse, {} no drawable geometry",
            self.meshes_not_found, self.mesh_parse_failed, self.mesh_no_drawable
        );
        if self.collision_nifs > 0 {
            println!(
                "  collision:     {} NIFs with Havok shapes",
                self.collision_nifs
            );
        }
        println!(
            "  skipped:       {} markers, {} disabled, {} no model, {} no data/base, {} other types, {} effects",
            cell.skipped_markers,
            cell.skipped_disabled,
            cell.skipped_no_model,
            cell.skipped_no_data + cell.skipped_no_base,
            cell.skipped_other_types,
            cell.skipped_effects
        );
        println!(
            "  shells skipped (hollow pieces, OBND > {:.0} m^3): {}",
            SHELL_VOLUME_LIMIT, self.skipped_shells
        );
        if !self.largest.is_empty() {
            println!("  largest placeholders:");
            for (volume, edid, size, pos) in &self.largest {
                println!(
                    "    {edid}: {:.1} m^3, size ({:.1}, {:.1}, {:.1}) m at ({:.1}, {:.1}, {:.1})",
                    volume, size.x, size.y, size.z, pos.x, pos.y, pos.z
                );
            }
        }
    }
}

/// OBND volume (m^3) above which a placeholder box is treated as a
/// hollow room shell and skipped instead of drawn solid.
const SHELL_VOLUME_LIMIT: f32 = 100.0;

/// Floor heights under each square metre of the room, built from
/// the real meshes at scene build (M8.1): the ground for gravity.
/// Each cell holds the sorted heights of up-facing triangle tops
/// covering it — the floor, stair treads, table tops. A query
/// returns the highest height at or below a probe height, or the
/// room's floor level when the cell has no triangles there.
///
/// Placeholders are not rasterized (the floor is real meshes now);
/// actors are not floors either. Walls and ceilings never enter
/// the grid — only triangles facing up (normal.y > 0.5) count.
#[derive(Resource)]
pub struct FloorGrid {
    /// World x/z of cell (0, 0)'s corner.
    origin_x: f32,
    origin_z: f32,
    /// Cell size in metres.
    cell: f32,
    cols: usize,
    rows: usize,
    /// Sorted floor heights per cell (ascending), one Vec per cell.
    heights: Vec<Vec<f32>>,
    /// Floor level when a cell has no triangles.
    fallback_y: f32,
}

impl FloorGrid {
    /// Cell size in metres. Floors are flat, so 1 m cells are
    /// plenty; stairs still work because each cell keeps *every*
    /// height in it, not just the highest.
    pub const CELL_SIZE: f32 = 1.0;

    /// v0.2.2: hard cap on grid cells. A degenerate bounds box
    /// (a bad OBND or a far-flung object) must not turn the grid
    /// into a multi-gigabyte allocation — the cell size grows
    /// until the grid fits. The inn is ~1,200 cells at 1 m.
    pub const MAX_CELLS: usize = 100_000;
    /// v0.2.2: per-cell height cap. A degenerate mesh must not
    /// stuff one cell with millions of heights.
    pub const MAX_HEIGHTS_PER_CELL: usize = 4_096;

    pub fn new(bounds: &RoomBounds) -> Self {
        let mut cell = Self::CELL_SIZE;
        let (mut cols, mut rows) = Self::dims(bounds, cell);
        while cols.saturating_mul(rows) > Self::MAX_CELLS {
            cell *= 2.0;
            (cols, rows) = Self::dims(bounds, cell);
        }
        if cell > Self::CELL_SIZE {
            crate::startup_log::log(&format!(
                "floor grid: bounds {cols}x{rows} at 1 m exceeded the {}-cell cap — cell size raised to {cell} m",
                Self::MAX_CELLS
            ));
        }
        FloorGrid {
            origin_x: bounds.min.x,
            origin_z: bounds.min.z,
            cell,
            cols,
            rows,
            heights: vec![Vec::new(); cols * rows],
            fallback_y: bounds.floor_y,
        }
    }

    fn dims(bounds: &RoomBounds, cell: f32) -> (usize, usize) {
        (
            ((bounds.max.x - bounds.min.x) / cell).ceil().max(1.0) as usize,
            ((bounds.max.z - bounds.min.z) / cell).ceil().max(1.0) as usize,
        )
    }

    /// Add one world-space triangle: if it faces up, its top
    /// becomes a floor height in every cell the triangle
    /// OVERLAPS — not just cells whose centre it covers. A
    /// porch deck's triangles must make the deck walkable even
    /// where they only clip a cell's corner (the inn-return bug:
    /// the arrival spawn's cell centre missed the deck, so the
    /// player fell to the street). The recorded height is the
    /// plane height at the cell centre, clamped to the
    /// triangle's own Y span — exact for flat floors, honest
    /// for slopes. Tradeoff, by design: a cell reads the height
    /// once ANY part of it overlaps, so the player can stand
    /// within a cell of a surface's edge (a 1 m halo around thin
    /// floors) — the alternative leaves real floors unwalkable.
    pub fn add_triangle(&mut self, a: Vec3, b: Vec3, c: Vec3) {
        let normal = (b - a).cross(c - a);
        if normal.y <= 0.0 {
            return;
        }
        let n = normal.normalize();
        // Up-facing enough to stand on (within ~60° of vertical).
        if n.y < 0.5 {
            return;
        }
        let min_x = a.x.min(b.x).min(c.x);
        let max_x = a.x.max(b.x).max(c.x);
        let min_z = a.z.min(b.z).min(c.z);
        let max_z = a.z.max(b.z).max(c.z);
        let tri_min_y = a.y.min(b.y).min(c.y);
        let tri_max_y = a.y.max(b.y).max(c.y);
        let c0 = self.col(min_x);
        let c1 = self.col(max_x);
        let r0 = self.row(min_z);
        let r1 = self.row(max_z);
        for r in r0..=r1 {
            for col in c0..=c1 {
                let cx = self.origin_x + (col as f32 + 0.5) * self.cell;
                let cz = self.origin_z + (r as f32 + 0.5) * self.cell;
                // Fast path: centre covered (the old rule).
                // Otherwise the SAT overlap test — a corner clip
                // still counts.
                let x0 = self.origin_x + col as f32 * self.cell;
                let z0 = self.origin_z + r as f32 * self.cell;
                let overlaps = point_in_triangle_2d(cx, cz, a, b, c)
                    || triangle_overlaps_cell_2d(a, b, c, x0, x0 + self.cell, z0, z0 + self.cell);
                if overlaps {
                    // Plane height at the cell centre, clamped
                    // to the triangle's Y span (a no-op when
                    // the centre is inside the triangle — the
                    // plane height there is already in range).
                    let h = (a.y - (n.x * (cx - a.x) + n.z * (cz - a.z)) / n.y)
                        .clamp(tri_min_y, tri_max_y);
                    // v0.2.2: per-cell cap (see MAX_HEIGHTS_PER_CELL).
                    let cell_heights = &mut self.heights[r * self.cols + col];
                    if cell_heights.len() < Self::MAX_HEIGHTS_PER_CELL {
                        cell_heights.push(h);
                    }
                }
            }
        }
    }

    /// Sort each cell's heights (call once after all triangles).
    /// Near-duplicates (within 1 cm) are dropped to keep the lists
    /// short — overlapping floor pieces share heights.
    pub fn finish(&mut self) {
        for cell in &mut self.heights {
            cell.sort_by(|x, y| x.partial_cmp(y).unwrap_or(Ordering::Equal));
            cell.dedup_by(|b, a| (*b - *a).abs() < 0.01);
        }
    }

    /// Highest floor height at (x, z) at or below `max_y`, or the
    /// room's floor level when the cell has no triangle there (or
    /// every triangle there is above the probe — standing under a
    /// table reads the floor under it, not the tabletop).
    pub fn ground_height(&self, x: f32, z: f32, max_y: f32) -> f32 {
        let col = self.col(x);
        let row = self.row(z);
        let cell = &self.heights[row * self.cols + col];
        match cell.binary_search_by(|h| h.partial_cmp(&max_y).unwrap_or(Ordering::Equal)) {
            Ok(i) => cell[i],
            Err(0) => self.fallback_y,
            Err(i) => cell[i - 1],
        }
    }

    fn col(&self, x: f32) -> usize {
        (((x - self.origin_x) / self.cell).floor() as i32).clamp(0, self.cols as i32 - 1) as usize
    }

    fn row(&self, z: f32) -> usize {
        (((z - self.origin_z) / self.cell).floor() as i32).clamp(0, self.rows as i32 - 1) as usize
    }

    /// The sorted floor heights in the cell containing (x, z)
    /// (empty when the cell has no triangles). Only meaningful
    /// after [`finish`] (the sort). The obstacle grid's
    /// post-process reads this to tell steps from walls.
    pub fn heights_at(&self, x: f32, z: f32) -> &[f32] {
        let col = self.col(x);
        let row = self.row(z);
        &self.heights[row * self.cols + col]
    }

    /// (cells with data, min height, max height) for the summary.
    pub fn stats(&self) -> (usize, f32, f32) {
        let mut cells = 0;
        let mut min_h = f32::INFINITY;
        let mut max_h = f32::NEG_INFINITY;
        for cell in &self.heights {
            if cell.is_empty() {
                continue;
            }
            cells += 1;
            min_h = min_h.min(cell[0]);
            max_h = max_h.max(cell[cell.len() - 1]);
        }
        if cells == 0 {
            (0, self.fallback_y, self.fallback_y)
        } else {
            (cells, min_h, max_h)
        }
    }
}

/// World-space triangles of a mesh under an entity transform.
/// Used to build the floor grid (M8.1). Takes raw positions +
/// indices rather than a Bevy mesh: our meshes use RENDER_WORLD
/// usage, so their vertex data is extracted to the RenderWorld
/// after the first frame and cannot be re-read from Assets — the
/// chunked build reuses cached meshes in later frames, so the
/// geometry is captured once at cache time and lives on the cache
/// entry (the spawn_chunk panic, 2026-11-15). Positions are the
/// Bevy-coords values convert baked in (NIF-local transform
/// applied); indices are always U32.
fn world_triangles(
    positions: &[[f32; 3]],
    indices: &[u32],
    transform: &bevy::transform::components::Transform,
) -> Vec<(Vec3, Vec3, Vec3)> {
    let matrix = transform.to_matrix();
    let world: Vec<Vec3> = positions
        .iter()
        .map(|p| matrix.transform_point3(Vec3::from(*p)))
        .collect();
    let mut tris = Vec::new();
    for t in indices.as_chunks::<3>().0 {
        if let (Some(&a), Some(&b), Some(&c)) = (
            world.get(t[0] as usize),
            world.get(t[1] as usize),
            world.get(t[2] as usize),
        ) {
            tris.push((a, b, c));
        }
    }
    tris
}

/// Pull the rasterization geometry out of a freshly built Bevy
/// mesh, before it moves into Assets (after that, the first
/// frame's extraction takes the vertex data to the RenderWorld
/// and it can no longer be read back). Returns (positions,
/// indices); empty when the mesh carries no usable geometry.
/// Indices are normalized to U32 (convert always emits U32; the
/// U16 arm is defensive).
fn mesh_geometry(mesh: &Mesh) -> (Vec<[f32; 3]>, Vec<u32>) {
    let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(p)) => p.clone(),
        _ => Vec::new(),
    };
    let indices = match mesh.indices() {
        Some(Indices::U32(v)) => v.clone(),
        Some(Indices::U16(v)) => v.iter().map(|&i| i as u32).collect(),
        None => Vec::new(),
    };
    (positions, indices)
}

/// Color for a placeholder box, by base record type.
fn placeholder_color(base_type: &str) -> Color {
    match base_type {
        "STAT" => Color::srgb(0.55, 0.55, 0.58),
        "FURN" => Color::srgb(0.55, 0.38, 0.22),
        "CONT" => Color::srgb(0.75, 0.60, 0.25),
        "DOOR" => Color::srgb(0.25, 0.40, 0.65),
        "ACTI" => Color::srgb(0.55, 0.35, 0.75),
        "MISC" | "INGR" | "ALCH" | "BOOK" | "WEAP" | "ARMO" | "AMMO" | "KEYM" | "SLGM" | "NOTE"
        | "SCRL" => Color::srgb(0.35, 0.65, 0.35),
        "LIGH" => Color::srgb(1.0, 0.85, 0.45),
        "MSTT" => Color::srgb(0.45, 0.55, 0.45),
        "FLOR" | "TREE" | "GRAS" => Color::srgb(0.30, 0.55, 0.30),
        _ => Color::srgb(0.80, 0.80, 0.80),
    }
}

/// Default placeholder size in metres when a base record has no
/// OBND, by base type.
fn default_size(base_type: &str) -> Vec3 {
    match base_type {
        "STAT" => Vec3::new(1.0, 1.0, 1.0),
        "FURN" => Vec3::new(1.5, 1.0, 1.0),
        "CONT" => Vec3::new(0.8, 0.6, 0.6),
        "DOOR" => Vec3::new(1.0, 2.2, 0.2),
        "LIGH" => Vec3::new(0.3, 0.4, 0.3),
        "ACTI" => Vec3::new(0.5, 0.5, 0.5),
        _ => Vec3::new(0.3, 0.3, 0.3),
    }
}

/// Build a cuboid mesh of `size`, offset so its centre is at
/// `center` (both in the object's local Bevy space). Used for
/// placeholders, whose OBND centre is rarely the model origin.
fn placeholder_mesh(size: Vec3, center: Vec3) -> Mesh {
    let mut mesh = Mesh::from(Cuboid::new(size.x, size.y, size.z));
    if let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
    {
        for p in positions.iter_mut() {
            p[0] += center.x;
            p[1] += center.y;
            p[2] += center.z;
        }
    }
    mesh
}

/// One drawable sub-mesh of a real model: its mesh, its material,
/// and — when the sub-mesh swings on a hinge (M15) — the hinge
/// pivot in Skyrim units, NIF-root space (the parent `Door` node's
/// composed translation, from `Mesh::parent_transform`), plus the
/// panel's swing scale (secret door: its own open angle / the
/// door's open angle, -1.0 for the opposite panel of a double
/// door; 1.0 for everything else). And — when the sub-mesh hangs
/// under a spinning node (mill wheel) — the NIF-local spin data
/// (pivot + axle in NIF-root space, angular velocity from the
/// NIF's keys), from which the scene build derives the world
/// pivot/axle per placement.
#[derive(Clone)]
struct CachedSubmesh {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    hinge_local: Option<[f32; 3]>,
    angle_scale: f32,
    spin_local: Option<mill::SpinLocal>,
    /// Sawmill blade (keyframed direct-controller data in
    /// NIF-root space), from which the scene build derives the
    /// world pivot/axle per placement. `None` for meshes whose
    /// parent node carries no direct controller.
    blade_local: Option<sawmill::BladeLocal>,
    /// Local-space vertex positions for floor-grid
    /// rasterization, captured once at cache time from the fresh
    /// mesh (Bevy coords, NIF-local transform already baked in —
    /// the same values the Bevy mesh carries). Arc: the cache
    /// entry is cloned per ref, and the geometry must not be
    /// copied with it.
    positions: Arc<Vec<[f32; 3]>>,
    /// Triangle indices into `positions`.
    indices: Arc<Vec<u32>>,
}

/// Material cache key: (diffuse path, gradient path, normal
/// path, alpha-cutoff bits, blend mode, unlit, emissive bits).
/// Every input to `real_material` that changes the built
/// material must be in the key, or two different materials
/// collide on one handle.
type MaterialCacheKey = (
    Option<String>,
    Option<String>,
    Option<String>,
    u32,
    AlphaBlend,
    bool,
    (u32, u32, u32),
);

/// Whether the scene builder loads normal maps. Absent =
/// enabled (the windowed path never inserts this — the phone
/// always loads normals). The screenshot path inserts it from
/// `--no-normal-maps`, a desktop-proof aid for the 6 GB
/// lavapipe box (see `Args::no_normal_maps`).
#[derive(Resource, Clone, Copy)]
pub struct NormalMaps(pub bool);

/// Which phase of the chunked scene build (the ANR fix) the
/// builder is in. The phases run in order and never go back —
/// the progress fraction is built on that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnPhase {
    /// The objects loop: one `PlacedObject` at a time, NIF
    /// parse + texture decode + entity spawn. The bulk of the
    /// build (the 22 s inn / 32 s exterior freeze was here).
    Objects,
    /// Exterior dressing: terrain bakes (one LAND cell at a
    /// time), then the sky dome + sun — or, indoors, the
    /// no-lights fill light when the cell has none.
    Dressing,
    /// The world-map bake (UI M2 follow-up): the baker drives
    /// one budget slice per frame (Collect -> Decode -> Sample
    /// -> Shade) until the map image + markers are built. Runs
    /// once per app run — on later scene loads the map is
    /// already built, there is no baker, and the phase is
    /// skipped. Must finish before InGame: the character menu
    /// can never see an unbaked map.
    MapBake,
    /// Finish: sort the floor grid, insert the Doors /
    /// FloorGrid / TerrainHeights resources, publish the
    /// summary. Fast — one frame.
    Finish,
    /// The build is complete; the state machine has moved on
    /// to InGame. `SpawnProgress` stays as a record (final
    /// counts, final phase) until the next scene overwrites it.
    Done,
}

/// Everything the chunked scene build owns, kept as a Resource
/// so it persists across frames (the ANR fix). The `Builder`
/// borrows this each frame plus the per-frame SystemParams
/// (`Commands`, asset stores, provider) — the Builder's borrows
/// were the structural blocker to chunking, so the split is:
/// owned state lives HERE, borrowed state is taken fresh per
/// frame in `spawn_chunk`.
#[derive(Resource)]
pub struct SpawnProgress {
    mesh_cache: HashMap<String, Vec<CachedSubmesh>>,
    /// NIF paths that parsed but held no drawable geometry
    /// (v0.4.7: editor markers, particle-only NIFs). Refs
    /// using them spawn nothing — no placeholder box. The
    /// empty result is cached in `mesh_cache` too, so the
    /// NIF is parsed once per scene, not once per ref.
    no_drawable_cache: HashSet<String>,
    /// Keyed by [`MaterialCacheKey`]: the same texture can feed
    /// an opaque mesh, an alpha-tested one, a blended one, and a
    /// gradient-multiplied one, and those need different
    /// materials. Cutoff bits 0 = no cutoff.
    material_cache: HashMap<MaterialCacheKey, Handle<StandardMaterial>>,
    placeholder_mesh_cache: HashMap<([i32; 3], [i32; 3]), Handle<Mesh>>,
    placeholder_mat_cache: HashMap<String, Handle<StandardMaterial>>,
    summary: SceneSummary,
    /// Floor grid (M8.1): every real mesh's triangles are
    /// rasterized into it as they spawn, one placement at a time.
    floor_grid: FloorGrid,
    /// Obstacle grid (per-object collision): steep collision
    /// triangles + primitive footprints, rasterized alongside
    /// the floor grid. Same bounds as the floor grid, 0.25 m
    /// cells (see `ObstacleGrid::CELL_SIZE`).
    obstacle_grid: ObstacleGrid,
    /// Per-NIF collision geometry (Bevy coords, NIF-local
    /// transform baked in), converted once at parse time.
    /// `None` = the NIF has no usable collision shapes (cached
    /// so the NIF is not re-parsed per ref).
    collision_cache: HashMap<String, Option<Arc<CachedCollision>>>,
    /// Per-model door animation (M15): the open angle + duration
    /// read from the model's own NiTransformData, keyed by NIF
    /// path. `None` = the model has no swinging parts.
    door_anim_cache: HashMap<String, Option<doors::DoorAnim>>,
    /// Doors found while spawning (M15): one entry per door ref,
    /// drained into the `Doors` resource when the build finishes.
    doors: Vec<doors::DoorRuntime>,
    /// Doors that start open (M15): refs the player left open in
    /// a previous visit (`DoorMemory`), or forced open by
    /// `--door-open` in the screenshot path.
    open_doors: HashSet<u32>,
    /// Shadow-casting point lights spawned so far (M16): the
    /// per-scene cap lives in `light_casts_shadows` — each one
    /// costs 6 cubemap faces on the phone.
    shadow_lights: usize,
    /// Whether to load normal maps (`NormalMaps` resource;
    /// false only with `--no-normal-maps`, a desktop-proof
    /// aid — see `NormalMaps`).
    load_normals: bool,
    /// Current phase (Objects -> Dressing -> MapBake -> Finish
    /// -> Done).
    pub phase: SpawnPhase,
    /// Index of the next object to spawn (`phase == Objects`).
    next_object: usize,
    /// Index of the next LAND cell to bake (`phase == Dressing`).
    next_land: usize,
    /// Water already spawned this build (`phase == Dressing`).
    /// The Dressing phase re-runs when the frame budget
    /// expires after `spawn_water` but before the phase
    /// flips — without this guard the water planes spawn
    /// again on the next frame (render logs showed 12, even
    /// 18, water cells instead of 6). Set once `spawn_water`
    /// runs; the terrain loop's `next_land` index is safe
    /// the same way, but the water call has no index.
    water_spawned: bool,
    /// "terrain done" already logged this build (`phase ==
    /// Dressing`). The terrain loop is index-safe, but the
    /// boundary log sits AFTER the loop — on a Dressing
    /// re-run the loop is skipped (next_land already at the
    /// end) and the bare log fired twice (proven on the
    /// after-ext render: two "terrain done (9/9)" lines
    /// 134 ms apart). Same guard pattern as the water.
    terrain_done_logged: bool,
    /// Total objects / LAND cells in this scene, for progress.
    total_objects: usize,
    total_lands: usize,
    /// Terrain-bake texture cache (exterior dressing): the bake
    /// for one LAND cell reuses the decoded base/layer textures
    /// of the last — it must persist across frames, so it lives
    /// here, not in the dressing call.
    tex_cache: HashMap<String, Option<std::sync::Arc<crate::vtxt::DecodedTexture>>>,
    /// Shared fallback material for LAND cells whose bake fails
    /// (one handle, created on the first dressing frame).
    fallback_material: Option<Handle<StandardMaterial>>,
    /// The world-map baker (MapBake phase). `Some` on the
    /// first scene load of the run (the map is not built
    /// yet), `None` afterwards — the phase is skipped when
    /// there is no baker.
    map_baker: Option<crate::character::MapBaker>,
}

impl SpawnProgress {
    pub fn new(
        bounds: &RoomBounds,
        open_doors: HashSet<u32>,
        load_normals: bool,
        total_objects: usize,
        total_lands: usize,
    ) -> Self {
        SpawnProgress {
            mesh_cache: HashMap::new(),
            no_drawable_cache: HashSet::new(),
            material_cache: HashMap::new(),
            placeholder_mesh_cache: HashMap::new(),
            placeholder_mat_cache: HashMap::new(),
            summary: SceneSummary {
                objects_total: total_objects,
                ..Default::default()
            },
            floor_grid: FloorGrid::new(bounds),
            obstacle_grid: ObstacleGrid::new(bounds),
            collision_cache: HashMap::new(),
            door_anim_cache: HashMap::new(),
            doors: Vec::new(),
            open_doors,
            shadow_lights: 0,
            load_normals,
            phase: SpawnPhase::Objects,
            next_object: 0,
            next_land: 0,
            water_spawned: false,
            terrain_done_logged: false,
            total_objects,
            total_lands,
            tex_cache: HashMap::new(),
            fallback_material: None,
            map_baker: None,
        }
    }

    /// Build progress, 0.0–1.0. Monotonic by construction: the
    /// phases only advance, and within a phase only the index
    /// advances. Weights: the objects loop is ~82% of the build
    /// (NIF parse + texture decode), terrain dressing ~8%, the
    /// map bake ~9%, the finish ~1%. The number is indicative;
    /// the progress TEXT (counts) is the honest part. The
    /// loading screen shows the text today; this is the
    /// fraction a progress bar reads when one is drawn — the
    /// tests pin the contract meanwhile.
    #[allow(dead_code)]
    pub fn progress(&self) -> f32 {
        match self.phase {
            SpawnPhase::Objects => {
                if self.total_objects == 0 {
                    0.82
                } else {
                    0.82 * self.next_object as f32 / self.total_objects as f32
                }
            }
            SpawnPhase::Dressing => {
                if self.total_lands == 0 {
                    0.90
                } else {
                    0.82 + 0.08 * self.next_land as f32 / self.total_lands as f32
                }
            }
            SpawnPhase::MapBake => {
                // No baker (map already built) reads as done.
                match &self.map_baker {
                    Some(baker) => 0.90 + 0.09 * baker.progress(),
                    None => 0.99,
                }
            }
            SpawnPhase::Finish => 0.99,
            SpawnPhase::Done => 1.0,
        }
    }

    /// One line for the loading screen: the phase + counts.
    pub fn progress_text(&self) -> String {
        match self.phase {
            SpawnPhase::Objects => format!(
                "Spawning objects {}/{}",
                self.next_object, self.total_objects
            ),
            SpawnPhase::Dressing => {
                if self.total_lands == 0 {
                    "Lighting the room...".to_string()
                } else {
                    format!("Building terrain {}/{}", self.next_land, self.total_lands)
                }
            }
            SpawnPhase::MapBake => match &self.map_baker {
                Some(baker) => baker.progress_text(),
                None => "Drawing the map...".to_string(),
            },
            SpawnPhase::Finish => "Finishing...".to_string(),
            SpawnPhase::Done => "Done".to_string(),
        }
    }

    /// Finish the build: sort the floor grid, merge the obstacle
    /// grid, print their stats and the ground height at the
    /// camera spawn, and hand back the summary and the grids
    /// (both become resources). The progress keeps fresh empty
    /// grids + a default summary afterwards — the build is done,
    /// the record lives in the resources.
    fn finish_grids(
        &mut self,
        cell: &CellData,
        bounds: &RoomBounds,
    ) -> (SceneSummary, FloorGrid, ObstacleGrid) {
        self.floor_grid.finish();
        self.obstacle_grid
            .finish(&self.floor_grid, crate::player::STEP_HEIGHT);
        let (cells, min_h, max_h) = self.floor_grid.stats();
        let (ocells, ointervals) = self.obstacle_grid.stats();
        let (eye, _) = screenshot_viewpoint(cell, bounds);
        let spawn_ground = self.floor_grid.ground_height(eye.x, eye.z, eye.y);
        println!(
            "floor grid: {cells} cells with data, heights {min_h:.2}..{max_h:.2} m, ground at spawn {spawn_ground:.2} m"
        );
        println!("obstacle grid: {ocells} cells with data, {ointervals} intervals");
        crate::startup_log::log(&format!(
            "FloorGrid built: {cells} cells with data, heights {min_h:.2}..{max_h:.2} m, ground at spawn {spawn_ground:.2} m"
        ));
        crate::startup_log::log(&format!(
            "ObstacleGrid built: {ocells} cells with data, {ointervals} intervals"
        ));
        let summary = std::mem::take(&mut self.summary);
        let grid = std::mem::replace(&mut self.floor_grid, FloorGrid::new(bounds));
        let obstacles = std::mem::replace(&mut self.obstacle_grid, ObstacleGrid::new(bounds));
        (summary, grid, obstacles)
    }
}

/// Per-frame time budget for the chunked scene build (the ANR
/// fix): ~8 ms of build work per frame, checked between objects
/// (a single object can overrun it — a tree NIF + textures is
/// one unit of work — but the ANR limit is 5 s, so an 8 ms
/// budget with occasional overruns is nowhere near it). A FIXED
/// object count would be wrong: per-object cost varies hugely
/// (a tree NIF + textures vs a pot). Overridden by the
/// `SpawnBudget` resource (a test/proof hook — the multi-frame
/// test sets it to zero so every frame spawns exactly one
/// object).
pub const SPAWN_CHUNK_BUDGET: Duration = Duration::from_millis(8);

/// Override for [`SPAWN_CHUNK_BUDGET`] (test/proof hook only —
/// no production path inserts it). The chunk test sets it to
/// `Duration::ZERO` so the build provably splits across frames.
#[derive(Resource)]
pub struct SpawnBudget(pub Duration);

/// Everything the scene builder needs, kept in one place so the
/// spawn logic stays readable. The owned state (caches, summary,
/// floor grid, doors) lives in the `SpawnProgress` resource and
/// persists across frames; the borrowed state (commands, asset
/// stores, provider) is taken fresh each frame by `spawn_chunk`
/// — that split is what makes the chunked build possible.
struct Builder<'a, 'w, 's> {
    commands: &'a mut Commands<'w, 's>,
    meshes: &'a mut Assets<Mesh>,
    materials: &'a mut Assets<StandardMaterial>,
    images: &'a mut Assets<Image>,
    assets: &'a mut AssetProvider,
    progress: &'a mut SpawnProgress,
}

/// The entity transform + optional `DoorPanel` for one sub-mesh
/// spawn (M15). A swinging sub-mesh (hinge present + door ref
/// known) gets a `DoorPanel` carrying its hinge world position and
/// closed transform, and spawns already swung when the door starts
/// open. Everything else spawns with the plain ref transform and
/// no panel.
fn door_panel_spawn(
    transform: bevy::transform::components::Transform,
    hinge_local: Option<[f32; 3]>,
    angle_scale: f32,
    door_ref: Option<u32>,
    starts_open: bool,
    anim: Option<doors::DoorAnim>,
) -> (
    bevy::transform::components::Transform,
    Option<doors::DoorPanel>,
) {
    let (Some(hinge_sky), Some(ref_form_id)) = (hinge_local, door_ref) else {
        return (transform, None);
    };
    let hinge_local_bevy = coords::to_bevy_position(hinge_sky);
    let hinge = transform.transform_point(hinge_local_bevy);
    let panel = doors::DoorPanel {
        ref_form_id,
        hinge,
        closed: transform,
        angle_scale,
    };
    let spawn_transform = if starts_open {
        let angle = anim.map(|a| a.open_angle).unwrap_or(0.0);
        doors::swing_transform(&transform, hinge, angle * angle_scale)
    } else {
        transform
    };
    (spawn_transform, Some(panel))
}

impl<'a, 'w, 's> Builder<'a, 'w, 's> {
    /// Material for a real mesh: textured when the NIF names a
    /// diffuse texture that resolves, plain light grey otherwise.
    /// M11: the base color is WHITE when a texture loads — the old
    /// 0.85 grey multiplied every texture by ~0.69 in linear light,
    /// darkening surfaces ~30% (part of the washed-out look). And
    /// when the shape carries an alpha-test cutoff (foliage needle
    /// cards), the material is alpha-to-coverage (M13), double-sided:
    /// without it the cards render as opaque paper sheets, and a
    /// plain mask hard-discards their edges (the wispy-pine look).
    /// Blend-only shapes (NiAlphaProperty bit 0 without bit 9 —
    /// fog, glass, water) get the transparent pass: `AlphaMode::Blend`
    /// (or `Add` for additive glows), double-sided, white base so
    /// the vertex colors carry the color. Effect-shader shapes
    /// (`unlit`) render without lighting — their color is the
    /// emissive color times the vertex colors, not the scene
    /// lights. A shape with BOTH a cutoff and a blend renders as
    /// cutout (cutoff wins — the blend is ignored). When the shape
    /// carries an effect-shader gradient texture, the diffuse is
    /// multiplied by it (the game's two-texture combine — without
    /// it the waterfall fog sheet reads as a flat slab). When the
    /// shape carries a normal map (`*_n.dds`, texture-set slot
    /// 1), it is wired as the material's normal map with the
    /// DirectX Y flip — without it every sunlit surface reads
    /// flat (the v0.4.4 phone report: "textures not right with
    /// the light"). The mesh tangents come from the NIF (see
    /// `convert.rs`); a mesh without tangents ignores the map.
    /// Emissive (forge milestone): the shape's
    /// `BSLightingShaderProperty` emissive color × multiple
    /// (linear light, resolved by dovah-nif) becomes the
    /// material's emissive — the forge coals (white × 2.39)
    /// glow instead of rendering flat. [0,0,0] is the common
    /// case and leaves the default black emissive alone.
    #[allow(clippy::too_many_arguments)]
    fn real_material(
        &mut self,
        diffuse: &Option<String>,
        gradient: &Option<String>,
        normal: &Option<String>,
        alpha_cutoff: Option<f32>,
        alpha_blend: AlphaBlend,
        unlit: bool,
        emissive: [f32; 3],
    ) -> Handle<StandardMaterial> {
        let key = (
            diffuse.clone(),
            gradient.clone(),
            normal.clone(),
            alpha_cutoff.map_or(0, f32::to_bits),
            alpha_blend,
            unlit,
            (
                emissive[0].to_bits(),
                emissive[1].to_bits(),
                emissive[2].to_bits(),
            ),
        );
        if let Some(entry) = self.progress.material_cache.get(&key) {
            return entry.clone();
        }
        let mut material = StandardMaterial::default();
        let mut textured = false;
        if let Some(path) = diffuse {
            let mut missing = false;
            if let Some(bytes) = self.assets.read_file(path) {
                let image = match gradient {
                    Some(grad_path) => match self.assets.read_file(grad_path) {
                        Some(grad_bytes) => {
                            self.progress.summary.textures_loaded += 1;
                            convert::dds_multiply_to_bevy_image(&bytes, &grad_bytes)
                        }
                        // No gradient bytes: plain source (the
                        // multiply also falls back on a bad
                        // gradient file — a missing gradient must
                        // not lose the fog).
                        None => convert::dds_to_bevy_image(&bytes),
                    },
                    None => convert::dds_to_bevy_image(&bytes),
                };
                if let Some(image) = image {
                    material.base_color_texture = Some(self.images.add(image));
                    self.progress.summary.textures_loaded += 1;
                    textured = true;
                } else {
                    missing = true;
                }
            } else {
                missing = true;
            }
            if missing {
                self.progress.summary.textures_missing += 1;
                if self.progress.summary.missing_textures.len() < 20
                    && !self.progress.summary.missing_textures.contains(path)
                {
                    self.progress.summary.missing_textures.push(path.clone());
                }
            }
        }
        // Normal map (slot 1 of the texture set): the surface
        // relief under the sun. Skyrim's maps are DirectX
        // (green = down), so the material flips Y in the
        // shader — Bevy's own flag for exactly this, verified
        // against the StoneFloor01_n data (the green channel
        // correlates -0.55 with the diffuse's vertical gradient
        // at the mortar joints: green encodes -B, the DirectX
        // convention). A missing/undecodable normal map is
        // silent — the surface just reads flat, as before.
        // Skipped entirely with `--no-normal-maps`.
        if self.progress.load_normals {
            if let Some(path) = normal {
                if let Some(bytes) = self.assets.read_file(path) {
                    if let Some(image) = convert::dds_normal_to_bevy_image(&bytes) {
                        material.normal_map_texture = Some(self.images.add(image));
                        material.flip_normal_map_y = true;
                        self.progress.summary.textures_loaded += 1;
                    }
                }
            }
        }
        material.base_color = if textured || alpha_blend != AlphaBlend::Opaque {
            // Blend shapes carry their color in the vertex colors
            // (fog is pure white with alpha) — the 0.85 grey would
            // dim them for no reason.
            Color::WHITE
        } else {
            Color::srgb(0.85, 0.85, 0.85)
        };
        // M1 (v0.4.13 phone feedback): flat matte roughness.
        // Bevy's StandardMaterial default is perceptual_roughness
        // 0.5 (semi-glossy), and nothing here ever set it — so
        // every NIF surface rendered semi-glossy. Two defects
        // traced to that, both proven from the vendored shader:
        // (1) the ambient SPECULAR term is albedo-independent
        // (pbr_ambient.wesl: EnvBRDFApprox(0.04, F_AB) ≈ 0.081
        // at roughness 0.5 vs ≈ 0.024 at 0.9), so on dark
        // surfaces the white specular ambient outshone the
        // colored diffuse ambient ~6× at exterior ambient 2400
        // — the grey wash; (2) the D_GGX specular highlight at
        // roughness 0.5 from a 30,000-cd point light is as
        // bright as the diffuse — the door glow, the bottle
        // white disc, the floating white discs. Skyrim's own
        // glossiness (BSLightingShaderProperty, 8–500 across
        // the Riverwood NIFs) is a subtle Gamebryo sheen, not a
        // PBR gloss — a flat 0.9 is the defensible default
        // until glossiness is plumbed through dovah-nif (a
        // follow-up: the Mesh struct doesn't carry it yet).
        material.perceptual_roughness = 0.9;
        if unlit {
            // Effect-shader shapes (fog, glows): no scene lighting.
            material.unlit = true;
        }
        if emissive != [0.0, 0.0, 0.0] {
            // Emissive glow (forge milestone): the NIF's
            // emissive color × multiple, already in linear
            // light — added to the lit color, so the shape
            // glows in darkness. Values above 1.0 are HDR
            // (the forge coals are white × 2.39).
            material.emissive = LinearRgba::rgb(emissive[0], emissive[1], emissive[2]);
        }
        if textured && alpha_cutoff.is_some() {
            // M1 (v0.4.13 phone feedback): binary alpha MASK at
            // the NIF's own threshold — NOT alpha-to-coverage.
            // M13 chose A2C to soften needle-card edges, but on
            // the phone (Bevy's Sample4 MSAA default) A2C's
            // partial coverage lets the BRIGHT SKY bleed through
            // the foliage, so the pines render as pale grey
            // wisps instead of trees — and A2C runs at a fixed
            // 0.5 cutoff (StandardMaterial has no alpha_cutoff
            // field; AlphaToCoverage carries no value), dropping
            // pixels in [0.439, 0.5) that the NIF's threshold
            // (112/255 = 0.439 for pine foliage) would keep.
            // Skyrim itself uses alpha test, so Mask(cutoff) is
            // the vanilla-faithful mode: binary on every
            // platform, the same foliage on desktop and phone.
            // (Desktop screenshots are 1-sample, so A2C
            // degraded to a binary 0.5 cutout there — desktop
            // renders UNDERSTATED the phone foliage problem.)
            material.alpha_mode = AlphaMode::Mask(alpha_cutoff.unwrap());
            material.double_sided = true;
        } else if alpha_blend != AlphaBlend::Opaque {
            // Transparent pass (exterior follow-up): blend-only
            // shapes — fog, glass, water — render in Bevy's
            // transparent queue, sorted back-to-front, instead
            // of as opaque slabs. The waterfall fog complex
            // (newFog, alpha 0.19–0.76) was a solid white plane
            // above the inn sign until this existed. Additive
            // shapes (glows, light shafts) add their color.
            // Vertex alpha carries the shape's transparency
            // (convert.rs passes it through for non-cutout
            // shapes); the base color is white so it does.
            material.alpha_mode = match alpha_blend {
                AlphaBlend::Blend => AlphaMode::Blend,
                AlphaBlend::Additive => AlphaMode::Add,
                AlphaBlend::Opaque => AlphaMode::Opaque,
            };
            material.double_sided = true;
        }
        let handle = self.materials.add(material);
        self.progress.material_cache.insert(key, handle.clone());
        handle
    }

    fn placeholder_material(&mut self, base_type: &str) -> Handle<StandardMaterial> {
        if let Some(entry) = self.progress.placeholder_mat_cache.get(base_type) {
            return entry.clone();
        }
        let handle = self.materials.add(StandardMaterial {
            base_color: placeholder_color(base_type),
            double_sided: true,
            ..Default::default()
        });
        self.progress
            .placeholder_mat_cache
            .insert(base_type.to_string(), handle.clone());
        handle
    }

    fn placeholder_mesh_handle(&mut self, size: Vec3, center: Vec3) -> Handle<Mesh> {
        // Quantize to centimetres so identical boxes share one mesh.
        let key = (
            [
                (size.x * 100.0) as i32,
                (size.y * 100.0) as i32,
                (size.z * 100.0) as i32,
            ],
            [
                (center.x * 100.0) as i32,
                (center.y * 100.0) as i32,
                (center.z * 100.0) as i32,
            ],
        );
        if let Some(handle) = self.progress.placeholder_mesh_cache.get(&key) {
            return handle.clone();
        }
        let handle = self.meshes.add(placeholder_mesh(size, center));
        self.progress
            .placeholder_mesh_cache
            .insert(key, handle.clone());
        handle
    }

    /// Rasterize one placement's collision geometry: floor
    /// triangles into the FloorGrid (when `rasterize` and the
    /// NIF has collision meshes — the "collision wins" rule: the
    /// game's physics walks on its collision, not its visuals,
    /// and the visual mesh skips stair treads), plus steep
    /// triangles and footprints into the ObstacleGrid (when
    /// `collide`).
    /// Returns whether the floor came from collision — the
    /// caller then skips visual floor rasterization for this
    /// placement.
    fn rasterize_collision(
        &mut self,
        collision: &Option<Arc<CachedCollision>>,
        transform: bevy::transform::components::Transform,
        rasterize: bool,
        collide: bool,
    ) -> bool {
        let Some(collision) = collision else {
            return false;
        };
        let use_collision_floor = rasterize && collision.has_mesh();
        if use_collision_floor {
            let tris = world_triangles(&collision.positions, &collision.floor_indices, &transform);
            for (a, b, c) in tris {
                self.progress.floor_grid.add_triangle(a, b, c);
            }
        }
        if collide && collision.has_obstacles() {
            let tris = world_triangles(
                &collision.positions,
                &collision.obstacle_indices,
                &transform,
            );
            for (a, b, c) in tris {
                self.progress.obstacle_grid.add_triangle(a, b, c);
            }
            for footprint in collision.footprints.iter() {
                let (min_x, max_x, min_z, max_z, min_y, max_y) =
                    transform_footprint(footprint, &transform);
                self.progress
                    .obstacle_grid
                    .add_footprint(min_x, max_x, min_z, max_z, min_y, max_y);
            }
        }
        use_collision_floor
    }

    /// Spawn one NIF's sub-meshes at `transform` (or all of its
    /// cached sub-meshes, when the model was already parsed).
    /// Returns the number of sub-meshes spawned: 0 when the file
    /// is missing, fails to parse, or holds no drawable geometry.
    /// The caller falls back to a placeholder only for a missing
    /// or unparseable model — a NIF that parsed but held no
    /// drawable geometry (editor markers, particle-only) spawns
    /// nothing (v0.4.7; see `no_drawable_cache`).
    /// Updates `meshes_not_found` / `mesh_parse_failed` /
    /// `real_submeshes`; the caller owns the per-object counting.
    /// `rasterize` feeds the placement's triangles into the floor
    /// grid — false for actors (people are not floors) and for
    /// trees/plants/grass. `collide` feeds the placement's
    /// collision into the obstacle grid — false for actors only
    /// (they move; their NIFs' bhk is ragdoll data). Trees DO
    /// collide: their trunk capsules block even though their
    /// treetops are never floors. When the NIF has collision
    /// meshes, the floor comes from them instead of the visuals
    /// (see [`rasterize_collision`]). Returns (sub-meshes spawned,
    /// swing info): the swing info is `Some` when the model has
    /// swinging parts (a sub-mesh under a hinge node), with the
    /// model's open animation. `door_ref` is the placed ref's
    /// form ID — swinging sub-meshes get a `DoorPanel` for it;
    /// `None` (actor parts) tags nothing.
    fn spawn_nif(
        &mut self,
        nif_path: &str,
        transform: bevy::transform::components::Transform,
        rasterize: bool,
        collide: bool,
        door_ref: Option<u32>,
        npc_ref: Option<u32>,
    ) -> (usize, Option<doors::NifSwing>) {
        // The door's start state: open when the player left it
        // open (or --door-open forced it). Panels spawn already
        // swung — no animation runs before the first frame.
        let starts_open = door_ref
            .map(|r| self.progress.open_doors.contains(&r))
            .unwrap_or(false);
        if self.progress.mesh_cache.contains_key(nif_path) {
            let cached = self
                .progress
                .mesh_cache
                .get(nif_path)
                .cloned()
                .unwrap_or_default();
            let collision = self
                .progress
                .collision_cache
                .get(nif_path)
                .cloned()
                .flatten();
            // Collision rasterizes even for a no-drawable NIF
            // (an invisible wall has collision but no meshes).
            let collision_floor =
                self.rasterize_collision(&collision, transform, rasterize, collide);
            let anim = self
                .progress
                .door_anim_cache
                .get(nif_path)
                .copied()
                .flatten();
            let mut spawned = 0;
            let mut swung = false;
            for entry in cached {
                // Floor grid (M8.1): rasterize this placement's
                // triangles under its own transform, from the
                // geometry captured at cache time — the Bevy
                // mesh's vertex data is extracted to the
                // RenderWorld after the first frame and cannot
                // be re-read from Assets (the spawn_chunk panic,
                // 2026-11-15). Skipped when the floor came from
                // the NIF's collision meshes.
                if rasterize && !collision_floor {
                    let tris = world_triangles(&entry.positions, &entry.indices, &transform);
                    for (a, b, c) in tris {
                        self.progress.floor_grid.add_triangle(a, b, c);
                    }
                }
                let (spawn_transform, panel) = door_panel_spawn(
                    transform,
                    entry.hinge_local,
                    entry.angle_scale,
                    door_ref,
                    starts_open,
                    anim,
                );
                if entry.hinge_local.is_some() {
                    swung = true;
                }
                // Mill wheel: the spin data was cached per
                // sub-mesh; the world pivot/axle derive from
                // this placement's transform.
                let wheel = entry
                    .spin_local
                    .map(|s| mill::mill_wheel_spawn(&spawn_transform, s));
                // Sawmill blade: same — the cached track plus
                // this placement's transform.
                let blade = entry
                    .blade_local
                    .as_ref()
                    .map(|b| sawmill::sawmill_blade_spawn(&spawn_transform, b));
                // Bevy 0.20 has no Bundle impl for Option<T>, so
                // panels spawn through the two-armed form; the
                // mill wheel is inserted after the spawn.
                let mut entity = if let Some(panel) = panel {
                    self.commands.spawn((
                        Mesh3d(entry.mesh),
                        MeshMaterial3d(entry.material),
                        spawn_transform,
                        panel,
                        SceneEntity,
                    ))
                } else {
                    self.commands.spawn((
                        Mesh3d(entry.mesh),
                        MeshMaterial3d(entry.material),
                        spawn_transform,
                        SceneEntity,
                    ))
                };
                if let Some(wheel) = wheel {
                    entity.insert(wheel);
                }
                if let Some(blade) = blade {
                    entity.insert(blade);
                }
                // Melee: every part of an actor carries the
                // actor's ref, so the death system can find the
                // parts to fall (cached path).
                if let Some(npc) = npc_ref {
                    entity.insert(crate::combat::NpcPart { ref_form_id: npc });
                }
                self.progress.summary.real_submeshes += 1;
                spawned += 1;
            }
            let swing = if swung {
                Some(doors::NifSwing {
                    anim: anim.unwrap_or_else(doors::DoorAnim::fallback),
                })
            } else {
                None
            };
            return (spawned, swing);
        }
        match self.assets.read_file(nif_path) {
            Some(bytes) => match dovah_nif::NifFile::from_bytes(&bytes) {
                Ok(nif) => {
                    // Per-object collision: convert the NIF's
                    // Havok shapes once, cache them, rasterize
                    // this placement — BEFORE the mesh loop, so
                    // a NIF with collision but no drawable
                    // geometry (an invisible wall) still
                    // collides, and the mesh loop knows whether
                    // the floor came from collision.
                    let collision = collision_from_shapes(&nif.collision_shapes()).map(Arc::new);
                    if collision.is_some() {
                        self.progress.summary.collision_nifs += 1;
                    }
                    let collision_floor =
                        self.rasterize_collision(&collision, transform, rasterize, collide);
                    self.progress
                        .collision_cache
                        .insert(nif_path.to_string(), collision);
                    // M15: the model's open animation, from its
                    // own NiTransformData (or the fallback when
                    // the NIF carries none).
                    let anim = doors::open_angle_from_nif(&nif).map(|(open_angle, duration)| {
                        doors::DoorAnim {
                            open_angle,
                            duration,
                        }
                    });
                    // Secret door: per-node open angles from the
                    // NIF's Open sequence — which panel gets
                    // which angle. Empty for NIFs without a
                    // parseable sequence (every panel then
                    // swings with the door, the M15 behavior).
                    let node_angles = nif.open_angles_by_node();
                    // Mill wheel: nodes the NIF spins through a
                    // full turn (the Idle sequence) — their
                    // meshes turn about the node's own origin.
                    // Keyed by node name for the mesh loop.
                    let spinning = nif.spinning_nodes();
                    let spins: HashMap<&str, &dovah_nif::SpinningNode> =
                        spinning.iter().map(|s| (s.node_name.as_str(), s)).collect();
                    // Sawmill blade: nodes the NIF animates
                    // through a direct NiTransformController
                    // (furniture) — their meshes turn about the
                    // node's own origin through the keyframed
                    // track. Keyed by node name for the mesh
                    // loop, like the wheel's spins.
                    let controllers = nif.direct_controllers();
                    let ctrls: HashMap<&str, &dovah_nif::DirectController> = controllers
                        .iter()
                        .map(|c| (c.node_name.as_str(), c))
                        .collect();
                    let mut entries = Vec::new();
                    let mut swung = false;
                    for sub in nif.meshes() {
                        // M15: a sub-mesh under a hinge node
                        // swings; the hinge pivot is the parent
                        // node's composed translation. Secret
                        // door: the panel's swing scale comes
                        // from its own Open angle over the
                        // door's — the opposite panel of a
                        // double door gets -1.0.
                        let (hinge_local, angle_scale) = if doors::is_hinge_node(&sub.parent_node) {
                            let scale = match anim {
                                Some(a) => doors::panel_angle_scale(
                                    node_angles.get(&sub.parent_node).map(|(angle, _)| *angle),
                                    a.open_angle,
                                ),
                                None => 1.0,
                            };
                            (Some(sub.parent_transform.translation), scale)
                        } else {
                            (None, 1.0)
                        };
                        // Mill wheel: a sub-mesh under a
                        // spinning node turns about the node's
                        // origin; the axle is the node's local
                        // spin axis mapped through the parent
                        // chain (the hinge pattern, generalized
                        // to an arbitrary axis).
                        let spin_local =
                            spins
                                .get(sub.parent_node.as_str())
                                .map(|spin| mill::SpinLocal {
                                    pivot: sub.parent_transform.translation,
                                    axle: sub.parent_transform.rotate(spin.axis_unit()),
                                    angular_velocity: spin.angular_velocity,
                                });
                        // Sawmill blade: a sub-mesh under a
                        // directly-controlled node turns about
                        // the node's origin through the
                        // controller's keyframed track (the
                        // driving axis — Y for the blade, X for
                        // the grindstone wheel). `driving_axis`
                        // returns None when no axis moves, so
                        // still controllers (the sash) attach
                        // nothing.
                        let blade_local = ctrls.get(sub.parent_node.as_str()).and_then(|ctl| {
                            sawmill::driving_axis(&ctl.data).map(|(axis, key_type)| {
                                let keys = match axis {
                                    0 => &ctl.data.x_keys,
                                    1 => &ctl.data.y_keys,
                                    _ => &ctl.data.z_keys,
                                };
                                sawmill::BladeLocal {
                                    pivot: sub.parent_transform.translation,
                                    axle: sub.parent_transform.rotate(sawmill::axis_unit(axis)),
                                    track: sawmill::BladeTrack {
                                        keys: keys.clone(),
                                        key_type,
                                        start_time: ctl.start_time,
                                        stop_time: ctl.stop_time,
                                        frequency: ctl.frequency,
                                        phase: ctl.phase,
                                    },
                                }
                            })
                        });
                        if let Some(mesh) = convert::nif_mesh_to_bevy(&sub) {
                            // Floor grid (M8.1): capture the
                            // rasterization geometry before the
                            // mesh moves into Assets — after that,
                            // the first frame's extraction takes
                            // the vertex data to the RenderWorld
                            // and it can no longer be read back.
                            let (positions, indices) = mesh_geometry(&mesh);
                            // Skipped when the floor came from
                            // the NIF's collision meshes.
                            if rasterize && !collision_floor {
                                let tris = world_triangles(&positions, &indices, &transform);
                                for (a, b, c) in tris {
                                    self.progress.floor_grid.add_triangle(a, b, c);
                                }
                            }
                            let material = self.real_material(
                                &sub.textures.diffuse,
                                &sub.textures.gradient,
                                &sub.textures.normal,
                                sub.alpha_cutoff,
                                sub.alpha_blend,
                                sub.unlit,
                                sub.emissive,
                            );
                            let handle = self.meshes.add(mesh);
                            let (spawn_transform, panel) = door_panel_spawn(
                                transform,
                                hinge_local,
                                angle_scale,
                                door_ref,
                                starts_open,
                                anim,
                            );
                            if hinge_local.is_some() {
                                swung = true;
                            }
                            entries.push(CachedSubmesh {
                                mesh: handle.clone(),
                                material: material.clone(),
                                hinge_local,
                                angle_scale,
                                spin_local,
                                blade_local: blade_local.clone(),
                                positions: Arc::new(positions),
                                indices: Arc::new(indices),
                            });
                            // Bevy 0.20 has no Bundle impl for
                            // Option<T>, so panels spawn through
                            // the two-armed form; the mill wheel
                            // (an independent optional component)
                            // is inserted after the spawn.
                            let wheel =
                                spin_local.map(|s| mill::mill_wheel_spawn(&spawn_transform, s));
                            // Sawmill blade: same shape — the
                            // world pivot/axle derive from this
                            // placement's transform.
                            let blade = blade_local
                                .as_ref()
                                .map(|b| sawmill::sawmill_blade_spawn(&spawn_transform, b));
                            let mut entity = if let Some(panel) = panel {
                                self.commands.spawn((
                                    Mesh3d(handle),
                                    MeshMaterial3d(material),
                                    spawn_transform,
                                    panel,
                                    SceneEntity,
                                ))
                            } else {
                                self.commands.spawn((
                                    Mesh3d(handle),
                                    MeshMaterial3d(material),
                                    spawn_transform,
                                    SceneEntity,
                                ))
                            };
                            if let Some(wheel) = wheel {
                                entity.insert(wheel);
                            }
                            if let Some(blade) = blade {
                                entity.insert(blade);
                            }
                            // Melee: every part of an actor
                            // carries the actor's ref, so the
                            // death system can find the parts to
                            // fall (fresh-parse path).
                            if let Some(npc) = npc_ref {
                                entity.insert(crate::combat::NpcPart { ref_form_id: npc });
                            }
                            self.progress.summary.real_submeshes += 1;
                        }
                    }
                    if entries.is_empty() {
                        // v0.4.7: the NIF parsed but held no
                        // drawable geometry (editor markers,
                        // particle-only) — the ref is invisible
                        // in the retail game, so the caller
                        // spawns nothing, not a placeholder.
                        // Cache the empty result so the NIF is
                        // parsed once per scene, not once per ref.
                        self.progress
                            .mesh_cache
                            .insert(nif_path.to_string(), Vec::new());
                        self.progress.no_drawable_cache.insert(nif_path.to_string());
                        (0, None)
                    } else {
                        let count = entries.len();
                        let swing = if swung {
                            Some(doors::NifSwing {
                                anim: anim.unwrap_or_else(doors::DoorAnim::fallback),
                            })
                        } else {
                            None
                        };
                        self.progress
                            .door_anim_cache
                            .insert(nif_path.to_string(), swing.map(|s| s.anim));
                        self.progress
                            .mesh_cache
                            .insert(nif_path.to_string(), entries);
                        (count, swing)
                    }
                }
                Err(_) => {
                    self.progress.summary.mesh_parse_failed += 1;
                    (0, None)
                }
            },
            None => {
                self.progress.summary.meshes_not_found += 1;
                (0, None)
            }
        }
    }

    /// Try the real mesh for an object; fall back to a placeholder
    /// box. Returns true when a real mesh was spawned.
    fn spawn_object(&mut self, object: &PlacedObject) {
        let position = coords::to_bevy_position(object.position);
        let rotation = coords::to_bevy_rotation(object.rotation);
        let transform = bevy::transform::components::Transform {
            translation: position,
            rotation,
            scale: Vec3::splat(object.scale),
        };
        // M9: trees, plants and grass are never floors — nobody
        // stands on a treetop — and their meshes carry the most
        // triangles outdoors (330 trees in Riverwood alone).
        // Skipping them from the floor grid keeps the exterior
        // build fast without changing where the player can walk.
        let rasterize = !matches!(object.base_type.as_str(), "TREE" | "FLOR" | "GRAS");

        if object.is_actor {
            // M12: actors render with their real NPC models — body,
            // outfit, and FaceGen head, resolved from the NPC_
            // record at load time. Parts share the mesh cache with
            // regular objects (both male NPCs share MaleBody_1)
            // and are never rasterized into the floor grid.
            self.progress.summary.actors += 1;
            let mut parts = 0;
            for model_path in &object.actor_models {
                let nif_path = format!("meshes\\{model_path}");
                if self
                    .spawn_nif(
                        &nif_path,
                        transform,
                        false,
                        false,
                        None,
                        Some(object.ref_form_id),
                    )
                    .0
                    > 0
                {
                    parts += 1;
                }
            }
            self.progress.summary.actor_parts += parts;
            if parts > 0 {
                return;
            }
            // Unresolvable NPC: the old red capsule, so the actor
            // is still visible where the game puts them.
            let mesh = self.meshes.add(Mesh::from(Capsule3d::new(0.25, 1.2)));
            let material = self.materials.add(StandardMaterial {
                base_color: Color::srgb(0.65, 0.15, 0.15),
                ..Default::default()
            });
            self.commands.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material),
                transform.with_translation(position + Vec3::new(0.0, 0.85, 0.0)),
                SceneEntity,
                // Melee: the capsule is an actor part too, so a
                // killed unresolvable NPC still falls.
                crate::combat::NpcPart {
                    ref_form_id: object.ref_form_id,
                },
            ));
            return;
        }

        // M14: model-less objects that reach here are interior
        // lights (the loader only keeps model-less actors —
        // handled above — and interior LIGH refs). They have no
        // mesh; the light itself is spawned by the caller's
        // `spawn_light`. Without this early return they fall
        // through to the placeholder box below and draw grey
        // cubes at every candle.
        let Some(model_path) = &object.model_path else {
            return;
        };
        let nif_path = format!("meshes\\{model_path}");
        let (spawned, swing) = self.spawn_nif(
            &nif_path,
            transform,
            rasterize,
            true,
            Some(object.ref_form_id),
            None,
        );
        if spawned > 0 {
            self.progress.summary.real_mesh_objects += 1;
            // M15: a swinging model is a door — record its runtime
            // state (plane geometry from the ref transform, width
            // from the OBND X extent, start state from memory).
            if let Some(swing) = swing {
                let width = object
                    .bounds
                    .map(|b| (b.max[0] - b.min[0]) as f32 / coords::SKYRIM_UNITS_PER_METER)
                    .filter(|w| *w > 0.0)
                    .unwrap_or(1.4);
                let starts_open = self.progress.open_doors.contains(&object.ref_form_id);
                self.progress.doors.push(doors::DoorRuntime::new(
                    object.ref_form_id,
                    &transform,
                    width,
                    object.xtel.is_some(),
                    swing.anim,
                    starts_open,
                ));
            }
            return;
        }
        // v0.4.7: a model that parsed but held no drawable
        // geometry (editor markers, particle-only) is
        // invisible in the retail game — spawn nothing, not
        // even a placeholder box. Placeholders stay for
        // models that are genuinely missing or unparseable.
        if self.progress.no_drawable_cache.contains(&nif_path) {
            self.progress.summary.mesh_no_drawable += 1;
            return;
        }
        self.spawn_placeholder(object, transform);
    }

    fn spawn_placeholder(
        &mut self,
        object: &PlacedObject,
        transform: bevy::transform::components::Transform,
    ) {
        // A placeholder box is a stand-in for a SOLID object. Room
        // shells (FarmIntInnEnd*, etc.) are hollow — walls, floor
        // and ceiling with an empty middle — so their OBND box is
        // not a shape, it is the room itself. Drawing it solid
        // swallows the whole interior. Skip boxes over the limit;
        // they come back as real meshes when the Meshes BSA lands.
        // Furniture tops out around a few m^3, room segments are
        // hundreds — 100 m^3 splits them cleanly for interiors.
        // (Exteriors may need revisiting: a solid boulder can be big.)
        let (size, center) = match object.bounds {
            Some(b) => {
                let min = [b.min[0] as f32, b.min[1] as f32, b.min[2] as f32];
                let max = [b.max[0] as f32, b.max[1] as f32, b.max[2] as f32];
                let (lo, hi) = coords::to_bevy_bounds(min, max);
                let size = (hi - lo).max(Vec3::splat(0.05));
                let center = (lo + hi) / 2.0;
                (size, center)
            }
            None => (default_size(&object.base_type), Vec3::ZERO),
        };
        if size.x * size.y * size.z > SHELL_VOLUME_LIMIT {
            self.progress.summary.skipped_shells += 1;
            return;
        }
        let mesh = self.placeholder_mesh_handle(size, center);
        let material = self.placeholder_material(&object.base_type);
        self.commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            transform,
            SceneEntity,
        ));
        // Track the largest boxes (top 5 by volume) for diagnostics.
        let volume = size.x * size.y * size.z;
        let entry = (
            volume,
            object.base_edid.clone(),
            size,
            transform.translation,
        );
        self.progress.summary.largest.push(entry);
        self.progress
            .summary
            .largest
            .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        self.progress.summary.largest.truncate(5);
        self.progress.summary.placeholder_objects += 1;
    }

    fn spawn_light(&mut self, object: &PlacedObject) {
        let Some(light) = object.light else {
            return;
        };
        let position = coords::to_bevy_position(object.position);
        let range = (light.radius as f32 / coords::SKYRIM_UNITS_PER_METER).clamp(1.0, 40.0);
        let color = Color::srgb_u8(light.color[0], light.color[1], light.color[2]);
        // M16: shadow-casting follows the LIGH type flag, capped
        // per scene — each shadow light costs 6 cubemap faces.
        let shadows = light_casts_shadows(&light, self.progress.shadow_lights);
        if shadows {
            self.progress.shadow_lights += 1;
        }
        self.commands.spawn((
            PointLight {
                intensity: skyrim_light_intensity(light.fade),
                range,
                color,
                shadow_maps_enabled: shadows,
                ..Default::default()
            },
            bevy::transform::components::Transform::from_translation(position),
            SceneEntity,
        ));
        // M1 light logging (defect 13): one line per active
        // light — ref, position, radius, color, fade, shadows.
        crate::startup_log::log(&format!(
            "light: ref {:#010x} pos ({:.1},{:.1},{:.1}) radius {:.1}m color #{:02x}{:02x}{:02x} fade {:.2} intensity {:.0} shadows {}",
            object.ref_form_id,
            position.x,
            position.y,
            position.z,
            range,
            light.color[0],
            light.color[1],
            light.color[2],
            light.fade,
            skyrim_light_intensity(light.fade),
            shadows,
        ));
        self.progress.summary.lights += 1;
    }
}

/// Sky dome radius in metres. The exterior block is ~175 m
/// across; the dome sits well outside it, inside the camera's
/// 1000 m far plane.
const SKY_DOME_RADIUS: f32 = 400.0;

/// Daylight sun illuminance in lux (a bright overcast noon is
/// ~10,000; clear noon is ~100,000 — the renderer is not
/// tone-mapped for full noon sun, so M9 uses a soft daylight).
const SUN_ILLUMINANCE: f32 = 12_000.0;

/// Interior ambient fill brightness. M14: 400 was the only light
/// before the real point lights existed — a flat white wash that
/// made the inn read grey. 50 still left a uniform light-grey
/// wash over the whole room (Dylan circled it: "light light grey
/// everywhere") — ambient × INDOOR exposure puts a grey floor
/// under every surface. M16 cut it to 10: the point lights do
/// all the work, shadowed corners go dark instead of grey, and
/// the remaining 10 is bounce light, not a wash. Tuned from the
/// M16 render.
///
/// v0.4.7: 10 is too dark NOW — the v0.4.6 linear-falloff fix
/// made the point lights ~12.6x brighter in effect, so the lit/
/// shadowed contrast is far higher than at M16, and Dylan's
/// phone shots show pitch-black corners, black table tops (flat
/// surfaces facing up, away from the low lights), and Orgnar's
/// face as a black void (he is lit only by the one overhead
/// light at (-16.55, 4.11, 0) + ambient — the fireplace is 11.4
/// m away, outside its 7.3 m radius). 40 restores the bounce
/// floor WITHOUT the wash because the fill is now WARM (see
/// `INTERIOR_AMBIENT_COLOR`): a warm fill reads as firelight
/// bounce, not grey. Render-proven against the M16 wash.
///
/// v0.4.10: 40 is STILL too dark — Dylan: "the shadows in the
/// inn are still too dark." The v0.4.7 fix only partly closed
/// the gap: the v0.4.6 linear-falloff lights are ~12.6x brighter
/// than the M16 lights the 40 was tuned against, so ambient 40
/// sits ~500x below a lit surface (30,000 x Fade candela at 2 m
/// is ~21,900 under the linear law, vs ambient 40) and
/// ambient-only surfaces — faces turned from the lights,
/// ceiling beams, the back wall — render at sRGB 25-40:
/// readable on a desktop monitor, black on a phone screen.
/// 200 was tried first and overshot: shadow pixels 35 -> 77
/// sRGB, but the whole room flattened (median 44 -> 85) — the
/// warm wash, the same failure mode as the M16 grey wash in a
/// warmer color. 100 is the calibrated middle: shadow pixels
/// ~57 sRGB (readable, no black voids) while the median stays
/// near 66 and the room keeps its depth. Measured on the
/// before/after counter + spawn renders.
const INTERIOR_AMBIENT_BRIGHTNESS: f32 = 100.0;

/// Interior ambient fill color (v0.4.7). The M16 grey wash came
/// from a WHITE ambient: it lifted every surface toward grey.
/// The inn's real lights are warm (FarmInteriorLight01
/// 0x64a2c6 = srgb (0.78, 0.64, 0.39); the fire 0x3467ad =
/// (0.68, 0.40, 0.20)), so the bounce fill is warm amber —
/// shadowed wood stays wood-colored, faces stay skin-colored,
/// and the room keeps its warm inn look while the corners read.
/// (M1: exteriors used to keep white too — see
/// EXTERIOR_AMBIENT_COLOR — but the white ambient was half the
/// exterior grey wash, so they get sky blue now.)
const INTERIOR_AMBIENT_COLOR: Color = Color::srgb(0.85, 0.62, 0.35);

/// Exterior ambient brightness (the sky fill). Was 800 — the
/// pines proved it too dim: shaded foliage read 0.23 sRGB with
/// 11% of pixels near-black and the trunk core black, because
/// the needle/bark albedos are ~0.02–0.03 linear (see the
/// `pine_light_test` diagnostic). Calibrated from that
/// diagnostic's matrix on the real treePineForest01.nif:
/// 800 → mean 0.233 / p5 0.061 / 11% black; 1600 → 0.313 /
/// 0.133 / 1.2%; 2400 → 0.368 / 0.184 / 0%; 3200 → 0.410 /
/// 0.226 / 0% (foliage starting to wash grey). 2400 is the
/// knee — shaded foliage and trunk read, sunlit brights barely
/// move (p95 0.413 → 0.482, sun-dominated). It is also the
/// physical clear-sky ratio: 2400 / 12000 lux sun = 20%.
/// Tonemapping was ruled OUT by the same matrix (AgX vs none vs
/// Reinhard: means within 0.016), and vertex colors barely
/// matter (vcolor off: 0.238 vs 0.233) — ambient was the knob.
const EXTERIOR_AMBIENT_BRIGHTNESS: f32 = 2400.0;

/// Exterior ambient fill color (M1, v0.4.13 phone feedback).
/// Was WHITE — and the white ambient was half the exterior grey
/// wash: the ambient specular term is albedo-independent
/// (pbr_ambient.wesl), so at roughness 0.5 a white ambient
/// 2400 washed every dark surface toward grey. The sky dome's
/// horizon color (0.75, 0.86, 0.96) is the physically right
/// fill for a clear-sky day — shadows go blue, as they do in
/// the vanilla Riverwood references, instead of grey. The
/// brightness stays 2400 pending a pine-matrix recalibration
/// at the new roughness (the old matrix was measured at
/// roughness 0.5, with the specular wash included).
const EXTERIOR_AMBIENT_COLOR: Color = Color::srgb(0.75, 0.86, 0.96);

/// Max shadow-casting point lights per scene (M16). Each one
/// renders 6 cubemap faces; the inn's data asks for 2 (the
/// fireplace + FarmInteriorShadowLight01), and 4 bounds a
/// pathological cell at 24 shadow views on the phone. Lights
/// past the cap still light the scene — they just don't cast.
const MAX_SHADOW_LIGHTS_PER_SCENE: usize = 4;

/// Point-light shadow map size in texels per cubemap face (M16).
/// Bevy's default is 1024; 512 quarters the shadow texels, and
/// at a 7.3 m range each face still resolves ~3 cm per texel at
/// the light's edge — fine for soft interior shadows on the
/// phone budget.
const POINT_LIGHT_SHADOW_MAP_SIZE: usize = 512;

/// Whether this light casts shadows: its LIGH base must carry
/// the Shadow Omnidirectional type flag (`LightData::
/// casts_shadows`) AND the scene must still have shadow budget
/// left. Pure so the cap is unit-testable.
fn light_casts_shadows(light: &crate::cell::LightData, shadow_lights_so_far: usize) -> bool {
    light.casts_shadows() && shadow_lights_so_far < MAX_SHADOW_LIGHTS_PER_SCENE
}

/// Point-light intensity per square metre of range (candela per
/// m² of the light's radius). A Skyrim light's radius is the
/// distance its glow reaches; scaling intensity with radius²
/// keeps the brightness at a fixed fraction of the radius
/// constant across light sizes. 40.0 puts a 512-unit (7.3 m)
/// inn light at ~2,138 cd: a table 2 m below it reads ~530 lux
/// (bright pool), 4 m away ~130 lux (readable mid-tones), at
/// the 7.3 m edge ~40 lux (dim). Tuned from the M14 render —
/// 8.0 left the room darker than the old ambient-only look
/// (mean 0.216 vs 0.305 sRGB), because inverse-square falloff
/// leaves most surfaces 3–4 m from the nearest light.
/// Point-light intensity per squared range (lumens per m²).
/// Skyrim's point-light falloff is LINEAR: attenuation(d) =
/// Fade x max(0, 1 - d/Radius), a hard cutoff at the light's
/// Radius. (Community Shaders ISL docs: "the engine's default
/// linear falloff"; Nexus ISL mod page: vanilla falloff has
/// F = fade, r = radius.) Bevy 0.20 ships inverse-square
/// falloff with no per-light override, so the vendored bevy_pbr
/// replaces getDistanceAttenuation with the linear shape (see
/// vendor/bevy_pbr/src/render/pbr_lighting.wesl). Under the
/// linear law the intensity does NOT scale with R^2 — a light's
/// brightness at a fixed fraction of its radius is already
/// fade-proportional — so intensity = K x Fade, where K is the
/// lux at the light's centre for Fade 1.0. The old law (8000 x
/// R^2, fireplace x2) is gone: the fireplace x2 was unfaithful
/// (all three inn light bases have FNAM Fade 1.0 —
/// ligh_data_dump), and the R^2 term only compensated the
/// inverse-square shape.
///
/// Note the 4pi: Bevy divides the point-light intensity by 4pi
/// on the CPU side when building the GPU light data
/// (bevy_pbr light.rs:574 — `intensity / (4.0 * PI)`), so the
/// shader's I/d^2 x window is really I/(4pi d^2) x window
/// against the value set here. The M14 "luminous power" model
/// was right; the division just lives in the upload, not the
/// shader. K below is the value set on the PointLight, so the
/// effective centre lux is K / 4pi.
///
/// K is calibrated by render, anchored by light_level_probe:
/// the probe's mean linear attenuation over the inn's 481 ref
/// positions is 0.445. The K=800 render came out at mean 0.122
/// (too dim) and K=1250 at 0.123 — nearly no response, because
/// the room's ~0.12 floor is the ambient (10.0), not the point
/// lights: at K=1250 the lights add only ~0.003 to the mean.
/// The old law's lights added ~0.070 (mean 0.190 - floor
/// 0.120), so matching that contribution needs K ~ 30,000.
/// The old law's max/mean shape ratio over the same points was
/// 79.7x; the linear law's is 3.5x — that ratio is the
/// blown-out door/wall bug.
const SKYRIM_LIGHT_LUX_SCALE: f32 = 30_000.0;

/// Skyrim's point-light attenuation at `distance_m` from a light
/// of radius `radius_m` and Fade `fade` — the CPU mirror of the
/// patched shader's getDistanceAttenuation, times Fade (the
/// shader takes Fade folded into the intensity). Pure —
/// unit-tested.
pub fn skyrim_falloff(distance_m: f32, radius_m: f32, fade: f32) -> f32 {
    fade * (1.0 - distance_m / radius_m).max(0.0)
}

/// Point-light intensity (lux at the light's centre) for a LIGH
/// base of the given Fade. Under the linear law the radius does
/// not enter the intensity — it only sets the cutoff.
fn skyrim_light_intensity(fade: f32) -> f32 {
    SKYRIM_LIGHT_LUX_SCALE * fade
}

/// Camera exposure for a scene: OVERCAST outdoors (matches the
/// 12,000 lux sun), INDOOR (ev100 7.0) indoors. M14: the
/// interior used BLENDER (ev100 9.7) with only the flat ambient
/// to light it — part of the grey-inn look. With real point
/// lights the indoor level reads warm at INDOOR.
fn exposure_for(exterior: bool) -> bevy::camera::Exposure {
    if exterior {
        bevy::camera::Exposure::OVERCAST
    } else {
        bevy::camera::Exposure::INDOOR
    }
}

/// Build the sky dome mesh: a sphere with a vertical vertex-color
/// gradient (deep blue at the zenith, pale at the horizon). Unlit
/// — the sky is a backdrop, not a lit surface.
fn sky_dome_mesh() -> Mesh {
    let mut mesh = Mesh::from(Sphere::new(SKY_DOME_RADIUS));
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION).cloned()
    else {
        return mesh;
    };
    let zenith = [0.20, 0.42, 0.80, 1.0];
    let horizon = [0.75, 0.86, 0.96, 1.0];
    let colors: Vec<[f32; 4]> = positions
        .iter()
        .map(|p| {
            // Normalized height: 0 at the horizon, 1 at the zenith.
            // Below the horizon stays horizon-colored (the dome's
            // lower half is under the terrain anyway).
            let t = (p[1] / SKY_DOME_RADIUS).clamp(0.0, 1.0);
            [
                horizon[0] + (zenith[0] - horizon[0]) * t,
                horizon[1] + (zenith[1] - horizon[1]) * t,
                horizon[2] + (zenith[2] - horizon[2]) * t,
                1.0,
            ]
        })
        .collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh
}

/// OnEnter(Spawning): set the scene's ambient + clear color,
/// the point-light shadow budget, and build the `SpawnProgress`
/// the chunk system drains. Instant — the work happens in
/// `spawn_chunk`, a budget at a time. (The old `spawn_cell` did
/// all of this synchronously in OnEnter(InGame): 22 s for the
/// inn, 32 s for the exterior, main thread, zero frames — the
/// ANR. The chunked build is the fix.)
// Bevy systems take their inputs as parameters by design; the
// count is the dependency list, not a smell to refactor away.
#[allow(clippy::too_many_arguments)]
pub fn begin_spawning(
    mut commands: Commands,
    cell: Res<CellData>,
    bounds: Res<RoomBounds>,
    mut ambient: ResMut<bevy::light::GlobalAmbientLight>,
    mut clear_color: ResMut<bevy::camera::ClearColor>,
    // M15: doors the player left open seed their start state.
    // Optional — the resource is inserted at app build, but a
    // caller that never built the windowed app must not panic.
    memory: Option<Res<doors::DoorMemory>>,
    // Exterior follow-up: normal maps off only with
    // `--no-normal-maps` (desktop-proof aid). Absent =
    // enabled — the windowed path never inserts this.
    normal_maps: Option<Res<NormalMaps>>,
    // The world map bakes once per app run: a baker is
    // created only while the MapIndex is unbuilt. Optional —
    // a caller without CharacterPlugin bakes by default (the
    // phase skips itself if the plugin or strings are
    // missing too).
    map: Option<Res<crate::character::MapIndex>>,
) {
    // v0.2.2: scene build is the heaviest main-thread step after
    // the load — log its boundaries and its counts.
    crate::startup_log::log_rss(&format!(
        "scene build start ({} objects)",
        cell.objects.len()
    ));
    let exterior = cell.is_exterior();
    // Daylight vs interior gloom: the ambient fill and the clear
    // color follow the scene, not the app (a door transition
    // flips them both).
    if exterior {
        ambient.brightness = EXTERIOR_AMBIENT_BRIGHTNESS;
        // M1: sky-blue fill, not white (see
        // EXTERIOR_AMBIENT_COLOR) — the white ambient was half
        // the exterior grey wash.
        ambient.color = EXTERIOR_AMBIENT_COLOR;
        clear_color.0 = Color::srgb(0.55, 0.74, 0.92);
    } else {
        ambient.brightness = INTERIOR_AMBIENT_BRIGHTNESS;
        // v0.4.7: warm bounce fill, not a white wash (see
        // INTERIOR_AMBIENT_COLOR). A door transition flips the
        // color back, so the two scenes never leak into each
        // other.
        ambient.color = INTERIOR_AMBIENT_COLOR;
        clear_color.0 = Color::srgb(0.08, 0.09, 0.12);
    }
    // M1 light logging (defect 13): the lighting state at scene
    // build, one line per scene — the next "too bright" or
    // "glowing door" report reads against this. Per-light lines
    // follow from spawn_light / spawn_sky_and_sun /
    // spawn_fill_light.
    let amb = ambient.color.to_srgba();
    crate::startup_log::log(&format!(
        "light: {} ambient {:.0} rgb({:.2},{:.2},{:.2}) ev100 {:.1}",
        if exterior { "exterior" } else { "interior" },
        ambient.brightness,
        amb.red,
        amb.green,
        amb.blue,
        exposure_for(exterior).ev100,
    ));
    // M16: point-light shadow map size (per cubemap face). The
    // exterior has no shadow point lights, so the resource is
    // inert outdoors; it is set here, next to the ambient, so
    // every scene load carries the phone budget.
    commands.insert_resource(bevy::light::PointLightShadowMap {
        size: POINT_LIGHT_SHADOW_MAP_SIZE,
    });
    let mut progress = SpawnProgress::new(
        &bounds,
        memory.map(|m| m.open.clone()).unwrap_or_default(),
        normal_maps.map(|n| n.0).unwrap_or(true),
        cell.objects.len(),
        cell.lands.len(),
    );
    // The world map bakes once per app run (the MapBake
    // phase): if it is already built, no baker is created
    // and the phase is skipped.
    if !map.as_deref().map(|m| m.built).unwrap_or(false) {
        progress.map_baker = Some(crate::character::MapBaker::new());
    }
    commands.insert_resource(progress);
}

/// The chunked scene build (the ANR fix): one Update system,
/// run while the app is in `Spawning`, that drains the
/// `SpawnProgress` a time budget at a time and flips to InGame
/// when the build is done. The Builder is rebuilt fresh each
/// frame over the progress (borrowed SystemParams) — the
/// progress is what persists. The phases run in order and never
/// go back: Objects (one `PlacedObject` per budget check) ->
/// Dressing (one LAND bake per budget check, then sky + sun or
/// the fill light) -> MapBake (one map-bake slice per budget
/// check; once per app run, skipped afterwards) -> Finish
/// (floor grid + resource insertion, one frame). The budget is
/// checked BETWEEN units of work, so a single expensive object
/// (a tree NIF + textures) can overrun it — the ANR limit is
/// 5 s, an 8 ms budget with occasional overruns is nowhere
/// near it. The loading screen stays alive throughout (the
/// windowed path despawns it on InGame entry), and the game
/// camera spawns after the build, so no half-built scene ever
/// renders.
// Bevy systems take their inputs as parameters by design; the
// count is the dependency list, not a smell to refactor away.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_chunk(
    mut commands: Commands,
    cell: Res<CellData>,
    bounds: Res<RoomBounds>,
    mut assets: ResMut<AssetProvider>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut summary: ResMut<SceneSummary>,
    // Optional: `begin_spawning` inserts the progress via
    // `Commands` in OnEnter(Spawning), and the first Update may
    // run before those commands flush — a required `ResMut`
    // would panic the app (the v0.2.2 lesson). Absent = skip
    // this frame; the progress is there by the next.
    progress: Option<ResMut<SpawnProgress>>,
    budget: Option<Res<SpawnBudget>>,
    // MapBake: the plugin (for the bake walk), the strings
    // (for marker names), and the map index (for the finished
    // bake). All optional — the phase skips itself when any
    // is missing, the same guard the old OnEnter builder had.
    plugin: Option<Res<crate::SharedPlugin>>,
    strings: Option<Res<crate::dialogue::GameStrings>>,
    mut map_index: Option<ResMut<crate::character::MapIndex>>,
    mut next_state: ResMut<NextState<crate::AppState>>,
) {
    let Some(mut progress) = progress else {
        return;
    };
    let budget = budget.map(|b| b.0).unwrap_or(SPAWN_CHUNK_BUDGET);
    let started = Instant::now();
    // Objects: the bulk of the build. Same order as the sync
    // path (cell order), same per-object work — the chunking
    // changes WHEN objects spawn, never WHAT spawns.
    if progress.phase == SpawnPhase::Objects {
        {
            let mut builder = Builder {
                commands: &mut commands,
                meshes: &mut meshes,
                materials: &mut materials,
                images: &mut images,
                assets: &mut assets,
                progress: &mut progress,
            };
            while builder.progress.next_object < cell.objects.len() {
                let object = &cell.objects[builder.progress.next_object];
                builder.spawn_object(object);
                if object.light.is_some() {
                    builder.spawn_light(object);
                }
                builder.progress.next_object += 1;
                if started.elapsed() >= budget {
                    return;
                }
            }
        }
        // Phase-boundary log (9/9 crash diagnosis): each
        // boundary line fires exactly once per build, so the
        // next crash log localizes the step even if Dylan
        // sends it late.
        crate::startup_log::log(&format!(
            "scene build: objects done ({} objects), dressing start",
            cell.objects.len()
        ));
        progress.phase = SpawnPhase::Dressing;
        if started.elapsed() >= budget {
            return;
        }
    }
    // Dressing: terrain bakes, then sky + sun (exterior) or the
    // fill light (interior, only when the cell has no lights —
    // the objects loop has counted them by now).
    if progress.phase == SpawnPhase::Dressing {
        if cell.is_exterior() {
            {
                let mut builder = Builder {
                    commands: &mut commands,
                    meshes: &mut meshes,
                    materials: &mut materials,
                    images: &mut images,
                    assets: &mut assets,
                    progress: &mut progress,
                };
                while builder.progress.next_land < cell.lands.len() {
                    let land = &cell.lands[builder.progress.next_land];
                    spawn_terrain_cell(&mut builder, land);
                    builder.progress.next_land += 1;
                    if started.elapsed() >= budget {
                        return;
                    }
                }
                // Phase-boundary log: the terrain loop exits
                // only when every LAND cell is baked. Guarded
                // like the water — the log sits after the
                // loop, so a Dressing re-run would fire it
                // again (the loop is skipped, next_land
                // already at the end).
                if !builder.progress.terrain_done_logged {
                    builder.progress.terrain_done_logged = true;
                    crate::startup_log::log(&format!(
                        "terrain done ({}/{})",
                        builder.progress.next_land,
                        cell.lands.len()
                    ));
                }
                // Water: flat planes at the resolved levels
                // (cheap — a handful of quads, one budget slice
                // for the whole set, not one cell per check).
                // Guarded: if the budget expires after this
                // call but before the phase flips, the next
                // frame re-runs Dressing — the guard keeps the
                // planes spawning once per build.
                if !builder.progress.water_spawned {
                    spawn_water(&mut builder, &cell);
                    builder.progress.water_spawned = true;
                    crate::startup_log::log(&format!(
                        "water done ({} cells)",
                        builder.progress.summary.water_cells
                    ));
                }
            }
            if started.elapsed() >= budget {
                return;
            }
            {
                let mut builder = Builder {
                    commands: &mut commands,
                    meshes: &mut meshes,
                    materials: &mut materials,
                    images: &mut images,
                    assets: &mut assets,
                    progress: &mut progress,
                };
                spawn_sky_and_sun(&mut builder, &bounds);
                crate::startup_log::log("sky done");
            }
        } else if progress.summary.lights == 0 {
            let mut builder = Builder {
                commands: &mut commands,
                meshes: &mut meshes,
                materials: &mut materials,
                images: &mut images,
                assets: &mut assets,
                progress: &mut progress,
            };
            spawn_fill_light(&mut builder, &bounds);
        }
        progress.phase = SpawnPhase::MapBake;
        if started.elapsed() >= budget {
            return;
        }
    }
    // MapBake: the world-map bake (UI M2 follow-up). The
    // one-shot bake took ~7 s on the main thread in
    // OnEnter(InGame) — an ANR on every first launch. The
    // baker does the same work a budget slice at a time; the
    // phase completes before InGame, so the character menu can
    // never see an unbaked map. On later scene loads the map
    // is already built (no baker) and the phase is skipped.
    if progress.phase == SpawnPhase::MapBake {
        let mut bake_done = progress.map_baker.is_none();
        if bake_done {
            // The map baked on an earlier scene load this run
            // (the inn's build, on a door transition out) —
            // the phase is a no-op, and the log says so.
            crate::startup_log::log("map bake skipped (already built)");
        }
        if !bake_done {
            match (
                plugin.as_deref(),
                strings.as_deref(),
                map_index.as_deref_mut(),
            ) {
                (Some(plugin), Some(strings), Some(map_index)) => {
                    let deadline = started + budget;
                    if progress
                        .map_baker
                        .as_ref()
                        .map(|b| b.is_fresh())
                        .unwrap_or(false)
                    {
                        crate::startup_log::log("map bake start (chunked, 8 ms budget)");
                    }
                    let finished = progress
                        .map_baker
                        .as_mut()
                        .map(|b| b.step(&plugin.0, deadline))
                        .unwrap_or(true);
                    if finished {
                        if let Some(baker) = progress.map_baker.take() {
                            let stats = baker.stats_line();
                            *map_index = baker.take_index(strings, &mut images);
                            crate::startup_log::log(&stats);
                        }
                        bake_done = true;
                    }
                }
                _ => {
                    // No plugin / strings / map resource —
                    // cannot bake (the same guard the old
                    // OnEnter builder had). Drop the baker and
                    // move on; the map stays unbuilt, as it
                    // would have been.
                    crate::startup_log::log(
                        "map bake skipped: plugin, strings, or map index missing",
                    );
                    progress.map_baker = None;
                    bake_done = true;
                }
            }
        }
        if bake_done {
            progress.phase = SpawnPhase::Finish;
        }
        if started.elapsed() >= budget {
            return;
        }
    }
    // Finish: sort the floor grid, insert the Doors / FloorGrid
    // / ObstacleGrid / TerrainHeights resources, publish the
    // summary. Fast —
    // one frame. Then InGame: the camera + catalog +
    // interactables + dialogue + quest index build there, in
    // the OnEnter(InGame) chain.
    if progress.phase == SpawnPhase::Finish {
        crate::startup_log::log("finish start");
        let exterior = cell.is_exterior();
        // M15: swinging doors found while spawning become the
        // Doors resource. Drained before `finish_grids`
        // takes the summary + grids out of the progress.
        // Re-inserted fresh on every scene load; the open/closed
        // memory lives in DoorMemory, not here.
        let doors = std::mem::take(&mut progress.doors);
        let door_count = doors.len();
        let (built_summary, grid, obstacles) = progress.finish_grids(&cell, &bounds);
        commands.insert_resource(doors::Doors {
            doors: doors.into_iter().map(|d| (d.ref_form_id, d)).collect(),
        });
        commands.insert_resource(grid);
        commands.insert_resource(obstacles);
        if exterior {
            commands.insert_resource(TerrainHeights::new(&cell.lands));
        } else {
            commands.remove_resource::<TerrainHeights>();
        }
        *summary = built_summary;
        crate::startup_log::log_rss(&format!(
            "scene build done: {} real-mesh objects ({} sub-meshes), {} placeholders, {} actors, {} lights, {} terrain cells, {} textures loaded, {} missing, {} doors",
            summary.real_mesh_objects,
            summary.real_submeshes,
            summary.placeholder_objects,
            summary.actors,
            summary.lights,
            summary.terrain_cells,
            summary.textures_loaded,
            summary.textures_missing,
            door_count
        ));
        summary.print(&cell);
        progress.phase = SpawnPhase::Done;
        next_state.set(crate::AppState::InGame);
    }
}

/// One LAND cell's terrain mesh + VTXT bake (exterior
/// dressing, chunked — one cell per budget check). The mesh
/// builds from the LAND heights; the bake resolves the base +
/// layer textures through the progress's texture cache, so a
/// cell reuses the last cell's decoded textures. A cell with no
/// resolvable base texture falls back to the shared white
/// material (VCLR only, the pre-M11 look).
fn spawn_terrain_cell(builder: &mut Builder<'_, '_, '_>, land: &LandData) {
    let mesh = land.terrain_mesh();
    let handle = builder.meshes.add(mesh);
    // The fallback material is created once, on the first
    // dressing frame, and shared by every failed bake — the
    // same single handle the sync path created up front.
    if builder.progress.fallback_material.is_none() {
        builder.progress.fallback_material = Some(builder.materials.add(StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 1.0,
            ..Default::default()
        }));
    }
    let material = match crate::vtxt::bake_terrain_texture(
        land,
        builder.assets,
        &mut builder.progress.tex_cache,
    ) {
        Some(baked) => {
            let image = builder.images.add(baked);
            builder.materials.add(StandardMaterial {
                base_color: Color::WHITE,
                base_color_texture: Some(image),
                perceptual_roughness: 1.0,
                ..Default::default()
            })
        }
        None => {
            // Diagnostic (v0.4.4 phone frames): a failed bake
            // leaves the whole cell as flat VCLR-tinted grey.
            // Log which cell and what its base texture was so
            // a missing LTEX/TXST/DDS is visible, not silent.
            println!(
                "terrain bake FAILED for cell {:?}: base_diffuse={:?}",
                land.grid, land.base_diffuse
            );
            builder
                .progress
                .fallback_material
                .clone()
                .expect("fallback material created above")
        }
    };
    builder.commands.spawn((
        Mesh3d(handle),
        MeshMaterial3d(material),
        bevy::transform::components::Transform::IDENTITY,
        SceneEntity,
    ));
    builder.progress.summary.terrain_cells += 1;
}

/// Water surfaces (exterior dressing, after the terrain):
/// one flat plane per `CellData.waters` entry, at the
/// resolved level, sharing one material per WATR type. The
/// material is the WATR deep colour at 60% alpha over the
/// procedural noise texture (so the flow stays visible even
/// with `--no-normal-maps`, which skips normal maps) plus the
/// WATR normal map when normals are enabled. The planes are
/// visual only — they never touch the floor/obstacle grids
/// (the player wades to the riverbed; swimming is later).
/// The `WaterFlow` component on each plane is what
/// `animate_water_flow` scrolls (see `water.rs`).
fn spawn_water(builder: &mut Builder<'_, '_, '_>, cell: &CellData) {
    if cell.waters.is_empty() {
        return;
    }
    let noise = builder.images.add(crate::water::water_noise_texture());
    let mut materials: HashMap<u32, Handle<StandardMaterial>> = HashMap::new();
    for surface in &cell.waters {
        let material = materials
            .entry(surface.watr.form_id)
            .or_insert_with(|| {
                let mut material = StandardMaterial {
                    base_color: crate::water::water_base_color(&surface.watr),
                    base_color_texture: Some(noise.clone()),
                    alpha_mode: AlphaMode::Blend,
                    metallic: 0.15,
                    perceptual_roughness: 0.06,
                    reflectance: 0.9,
                    ..Default::default()
                };
                // The WATR normal map (DefaultWater.dds for the
                // river types): ripple detail under the sun.
                // Same loader + DirectX flip as every NIF
                // normal map, and skipped entirely with
                // `--no-normal-maps` (the noise texture still
                // shows the flow). A missing/undecodable map is
                // silent — the water just reads flat.
                if builder.progress.load_normals {
                    if let Some(path) = &surface.watr.normal_map {
                        if let Some(bytes) = builder.assets.read_file(path) {
                            if let Some(image) = crate::convert::dds_normal_to_bevy_image(&bytes) {
                                material.normal_map_texture = Some(builder.images.add(image));
                                material.flip_normal_map_y = true;
                                builder.progress.summary.textures_loaded += 1;
                            }
                        }
                    }
                }
                builder.materials.add(material)
            })
            .clone();
        let (mesh, base_uvs) = crate::water::water_plane_mesh(surface.grid, surface.level);
        let flow = crate::water::WaterFlow::from_watr_flow(surface.watr.flow, base_uvs);
        builder.commands.spawn((
            Mesh3d(builder.meshes.add(mesh)),
            MeshMaterial3d(material),
            bevy::transform::components::Transform::IDENTITY,
            flow,
            SceneEntity,
        ));
        builder.progress.summary.water_cells += 1;
    }
}

/// Sky dome + sun (exterior dressing, after the terrain): the
/// dome is centred on the block so the horizon sits at eye
/// level from anywhere inside it — unlit, double-sided (the
/// camera is inside the sphere). The sun is a directional light
/// from the south-east, elevated about 50 degrees. No shadows
/// in M9 (shadow maps over a 175 m block need tuning; the
/// village reads fine unshadowed). Fast — one frame.
fn spawn_sky_and_sun(builder: &mut Builder<'_, '_, '_>, bounds: &RoomBounds) {
    let dome = sky_dome_mesh();
    let dome_handle = builder.meshes.add(dome);
    let dome_material = builder.materials.add(StandardMaterial {
        base_color: Color::WHITE,
        unlit: true,
        cull_mode: None,
        ..Default::default()
    });
    let center = bounds.center();
    builder.commands.spawn((
        Mesh3d(dome_handle),
        MeshMaterial3d(dome_material),
        bevy::transform::components::Transform::from_translation(Vec3::new(
            center.x, 0.0, center.z,
        )),
        SceneEntity,
    ));
    let sun_transform =
        bevy::transform::components::Transform::from_translation(Vec3::new(100.0, 150.0, 100.0))
            .looking_at(Vec3::ZERO, Vec3::Y);
    builder.commands.spawn((
        DirectionalLight {
            illuminance: SUN_ILLUMINANCE,
            color: Color::srgb(1.0, 0.96, 0.88),
            shadow_maps_enabled: false,
            ..Default::default()
        },
        sun_transform,
        SceneEntity,
    ));
    // M1 light logging (defect 13): the sun, one line.
    crate::startup_log::log(&format!(
        "light: sun illuminance {:.0} lux color #fff5e0 shadows false",
        SUN_ILLUMINANCE
    ));
}

/// Central fill light for a cell with no lights at all, so the
/// room is visible instead of pitch black. The sync path put
/// this inline in `spawn_cell`; the chunked path calls it from
/// the Dressing phase, after the objects loop has counted the
/// lights.
///
/// No ground slab: M6 added a placeholder floor because the room
/// shells were skipped and no floor mesh existed. M6.1 renders the
/// real shell and floor pieces, so the slab would only hide real
/// geometry (and z-fight where its top meets the real floor).
fn spawn_fill_light(builder: &mut Builder<'_, '_, '_>, bounds: &RoomBounds) {
    let center = bounds.center() + Vec3::new(0.0, 2.0, 0.0);
    // Lightless interiors only (never the exterior, never a lit
    // cell). Intensity 40 under the linear falloff law: ~35 lux
    // at 5 m, a gentle even lift so an unlit room is navigable
    // without reading as lit. The old 200 was tuned for
    // inverse-square (8 lux at 5 m); under linear falloff it
    // would have flooded the room at 175 lux.
    builder.commands.spawn((
        PointLight {
            intensity: 40.0,
            range: 40.0,
            color: Color::srgb(1.0, 0.9, 0.75),
            ..Default::default()
        },
        bevy::transform::components::Transform::from_translation(center),
        SceneEntity,
    ));
    // M1 light logging (defect 13): the fill light, one line.
    crate::startup_log::log(&format!(
        "light: fill pos ({:.1},{:.1},{:.1}) intensity 40 range 40m color #ffe6bf shadows false",
        center.x, center.y, center.z
    ));
    builder.progress.summary.lights += 1;
}

/// Camera viewpoint for the screenshot: an elevated corner view of
/// the main room, looking at its centre. Returns (position, look-at).
pub fn screenshot_viewpoint(cell: &CellData, bounds: &RoomBounds) -> (Vec3, Vec3) {
    let center = coords::to_bevy_position(cell.median_position());
    let size = bounds.size();
    // Eye-level view from inside the room: stand a little back from
    // the room's centre along its longest horizontal axis and look
    // across it — the way you would walking in the door. A high
    // corner view (tried first) ends up inside the room-shell
    // geometry, which occludes the whole shot.
    let long_axis = if size.x >= size.z { Vec3::X } else { Vec3::Z };
    // 0.12 of the long axis: far enough back to see the room, close
    // enough not to end up wedged against the door frame or a wall
    // piece (0.18 put the camera right next to the entrance door).
    let back = long_axis * size.x.max(size.z) * 0.12;
    let eye = center - back + Vec3::new(0.0, 1.7, 0.0);
    let look_at = center + back * 0.5 + Vec3::new(0.0, 0.2, 0.0);
    (eye, look_at)
}

/// Marker component for the player camera.
#[derive(Component)]
pub struct PlayerCamera;

/// Marker for the first-person controller (see `player` module).
#[derive(Component)]
pub struct Player;

/// Camera viewpoint from a spawn point (M9): stand at the XTEL
/// arrival position at eye height, facing the arrival yaw. A
/// Skyrim yaw of 0 faces north (+Y in Skyrim space) and yaw
/// increases clockwise seen from above, so the forward vector
/// is (sin yaw, cos yaw, 0) in Skyrim space.
fn spawn_viewpoint(spawn: &crate::cell::SpawnPoint) -> (Vec3, Vec3) {
    let eye =
        coords::to_bevy_position(spawn.position) + Vec3::new(0.0, crate::player::EYE_HEIGHT, 0.0);
    // Skyrim yaw is CLOCKWISE seen from above (north up): yaw 0
    // faces north (+Y) and positive yaw turns toward east, so the
    // forward vector is (sin yaw, cos yaw). The counterclockwise
    // reading (-sin yaw, cos yaw) was wrong — proven by the M9
    // screenshot: the inn door's XTEL yaw (-0.842) must face
    // north-west, out to the Riverwood road, but the old formula
    // faced north-east, into the inn's west wall a metre away.
    let forward_sky = [spawn.yaw.sin(), spawn.yaw.cos(), 0.0];
    let forward = coords::to_bevy_direction(forward_sky);
    (eye, eye + forward * 10.0)
}

/// Screenshot camera override (M12 proof hook): `--camera
/// x,y,z:lx,ly,lz` pins the viewpoint in Bevy metres, so a shot can
/// close in on one NPC, door, or corner instead of the default room
/// view. Inserted only on the screenshot path; absent everywhere
/// else, where `spawn_camera` falls back to the spawn/default
/// viewpoint.
#[derive(Resource)]
pub struct CameraOverride {
    pub position: Vec3,
    pub look_at: Vec3,
}

pub fn spawn_camera(
    mut commands: Commands,
    cell: Res<CellData>,
    bounds: Res<RoomBounds>,
    target: Option<Res<crate::screenshot::ScreenshotTarget>>,
    camera_override: Option<Res<CameraOverride>>,
) {
    // Phase-boundary log (9/9 crash diagnosis): spawn_camera
    // is the first real work in the OnEnter(InGame) chain, so
    // if the next crash log ends here the crash is in the
    // InGame entry chain, not the scene build.
    crate::startup_log::log("ingame enter (camera + indexes)");
    // A `--camera` override pins the viewpoint exactly; a spawn
    // point (door arrival) pins it next; without either the
    // interior viewpoint frames the room.
    let (position, look_at) = match &camera_override {
        Some(ov) => (ov.position, ov.look_at),
        None => match &cell.spawn {
            Some(spawn) => spawn_viewpoint(spawn),
            None => screenshot_viewpoint(&cell, &bounds),
        },
    };
    let transform = bevy::transform::components::Transform::from_translation(position)
        .looking_at(look_at, Vec3::Y);
    let angles = crate::player::LookAngles::from_forward(look_at - position);
    // Wide first-person FOV (Bevy's default is 45°, too narrow for
    // an interior). `Projection` is its own component in 0.20.
    let projection = bevy::camera::Projection::Perspective(bevy::camera::PerspectiveProjection {
        fov: 70f32.to_radians(),
        ..Default::default()
    });
    // The render target goes in the spawn bundle itself (same as
    // Bevy's render_to_texture example), not a follow-up insert.
    // `IsDefaultUiCamera` (M11): UI roots without an explicit
    // target render to the default UI camera, which Bevy picks as
    // the highest-order camera targeting the primary window — the
    // screenshot camera renders to an image, so without this
    // marker the `--screenshot-ui` HUD has no target camera and
    // silently never renders. On the windowed path the marker
    // names the same camera the fallback would pick anyway.
    // M11 exposure: the camera defaults to Exposure::BLENDER
    // (ev100 9.7), which overexposes the 12,000 lux outdoor sun by
    // ~5 stops (~39x) — the washed-out grey look. M14: indoors
    // use INDOOR (ev100 7.0) now that real point lights exist;
    // BLENDER + flat ambient was part of the grey-inn look.
    let exposure = exposure_for(cell.is_exterior());
    match target {
        Some(target) => {
            commands.spawn((
                bevy::camera::Camera3d::default(),
                bevy::camera::RenderTarget::Image(target.image.clone().into()),
                bevy::ui::IsDefaultUiCamera,
                exposure,
                projection,
                transform,
                PlayerCamera,
                Player,
                crate::player::PlayerPhysics::default(),
                angles,
            ));
        }
        None => {
            commands.spawn((
                bevy::camera::Camera3d::default(),
                bevy::ui::IsDefaultUiCamera,
                exposure,
                projection,
                transform,
                PlayerCamera,
                Player,
                crate::player::PlayerPhysics::default(),
                angles,
            ));
        }
    }
}

/// TEMPORARY M14 debug: print every point light's render state
/// (position, intensity, range, ViewVisibility) on the 3rd
/// frame, after the visibility pass has run. Proves whether
/// the lights reach the renderer at all. DELETE before M14
/// ships.
#[cfg(test)]
mod tests {
    use super::*;

    fn test_bounds() -> RoomBounds {
        RoomBounds {
            min: Vec3::new(-5.0, 0.0, -5.0),
            max: Vec3::new(5.0, 5.0, 5.0),
            floor_y: 0.0,
        }
    }

    /// A flat 4x4 m floor triangle pair at y = 0, covering x/z -2..2.
    /// Wound so the geometric normal points up (a -> b -> c is
    /// counterclockwise viewed from above in x/z).
    fn flat_floor(grid: &mut FloorGrid) {
        grid.add_triangle(
            Vec3::new(-2.0, 0.0, -2.0),
            Vec3::new(-2.0, 0.0, 2.0),
            Vec3::new(2.0, 0.0, 2.0),
        );
        grid.add_triangle(
            Vec3::new(-2.0, 0.0, -2.0),
            Vec3::new(2.0, 0.0, 2.0),
            Vec3::new(2.0, 0.0, -2.0),
        );
    }

    #[test]
    fn ground_height_reads_the_floor() {
        let mut grid = FloorGrid::new(&test_bounds());
        flat_floor(&mut grid);
        grid.finish();
        // Centre of the floor: ground is 0.
        assert_eq!(grid.ground_height(0.0, 0.0, 5.0), 0.0);
        // Outside the floor: the fallback.
        assert_eq!(grid.ground_height(4.5, 4.5, 5.0), 0.0);
    }

    #[test]
    fn ground_height_picks_highest_below_probe() {
        let mut grid = FloorGrid::new(&test_bounds());
        flat_floor(&mut grid);
        // A tabletop at y = 1 over the same cells.
        grid.add_triangle(
            Vec3::new(-1.0, 1.0, -1.0),
            Vec3::new(-1.0, 1.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
        );
        grid.add_triangle(
            Vec3::new(-1.0, 1.0, -1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(1.0, 1.0, -1.0),
        );
        grid.finish();
        // Probe above the table: the tabletop.
        assert_eq!(grid.ground_height(0.0, 0.0, 2.0), 1.0);
        // Probe below the table (standing under it): the floor.
        assert_eq!(grid.ground_height(0.0, 0.0, 0.5), 0.0);
    }

    #[test]
    fn corner_clipped_triangle_still_rasterizes() {
        // The inn-return bug: a triangle covering only the
        // corner of a cell must still make that cell walkable.
        // Triangle at y = 1 over x/z 0.9..2.0 — the corner of
        // cell [0,1]x[0,1], whose centre (0.5, 0.5) is outside
        // the triangle (x < 0.9).
        let mut grid = FloorGrid::new(&test_bounds());
        grid.add_triangle(
            Vec3::new(0.9, 1.0, 0.9),
            Vec3::new(0.9, 1.0, 2.0),
            Vec3::new(2.0, 1.0, 0.9),
        );
        grid.finish();
        // The overlapped corner reads the triangle.
        assert_eq!(grid.ground_height(0.95, 0.95, 5.0), 1.0);
        // The whole cell reads it (the 1 m halo — by design).
        assert_eq!(grid.ground_height(0.5, 0.5, 5.0), 1.0);
        // A cell in the bounding box the triangle does NOT
        // overlap gets nothing (the fallback).
        assert_eq!(grid.ground_height(2.5, 2.5, 5.0), 0.0);
    }

    #[test]
    fn overlap_height_clamps_to_triangle_span() {
        // Sloped triangle, plane y = x/2, Y span [0, 1].
        // The cell [2,3]x[0,1] only touches the triangle at its
        // high vertex, and the plane extrapolated to that cell's
        // centre (2.5, 0.5) would read 1.25 — the clamp keeps
        // the recorded height inside the triangle's span.
        let mut grid = FloorGrid::new(&test_bounds());
        grid.add_triangle(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
            Vec3::new(2.0, 1.0, 0.0),
        );
        grid.finish();
        assert_eq!(grid.ground_height(2.5, 0.5, 5.0), 1.0);
        // Centre-covered cells are unaffected by the clamp.
        assert_eq!(grid.ground_height(0.5, 0.5, 5.0), 0.25);
    }

    #[test]
    fn sat_overlap_cases() {
        let a = Vec3::new(0.9, 1.0, 0.9);
        let b = Vec3::new(0.9, 1.0, 2.0);
        let c = Vec3::new(2.0, 1.0, 0.9);
        // Corner clip: overlaps.
        assert!(triangle_overlaps_cell_2d(a, b, c, 0.0, 1.0, 0.0, 1.0));
        // Fully covered cell: overlaps.
        assert!(triangle_overlaps_cell_2d(a, b, c, 1.0, 2.0, 1.0, 2.0));
        // Disjoint cell (in the bbox, past the diagonal): no.
        assert!(!triangle_overlaps_cell_2d(a, b, c, 2.0, 3.0, 2.0, 3.0));
        // Far away: no.
        assert!(!triangle_overlaps_cell_2d(a, b, c, 5.0, 6.0, 5.0, 6.0));
    }

    #[test]
    fn downward_and_vertical_faces_are_not_floors() {
        let mut grid = FloorGrid::new(&test_bounds());
        // A downward-facing triangle (ceiling): wound so the normal
        // points down.
        grid.add_triangle(
            Vec3::new(-2.0, 3.0, -2.0),
            Vec3::new(2.0, 3.0, 2.0),
            Vec3::new(-2.0, 3.0, 2.0),
        );
        // A vertical triangle (wall).
        grid.add_triangle(
            Vec3::new(-2.0, 0.0, -2.0),
            Vec3::new(-2.0, 3.0, -2.0),
            Vec3::new(2.0, 3.0, -2.0),
        );
        grid.finish();
        let (cells, _, _) = grid.stats();
        assert_eq!(cells, 0);
        assert_eq!(grid.ground_height(0.0, 0.0, 5.0), 0.0);
    }

    #[test]
    fn huge_bounds_raise_the_cell_size_instead_of_exploding() {
        // v0.2.2: a degenerate bounds box (a bad OBND, a far-flung
        // object) must not allocate millions of grid cells.
        let bounds = RoomBounds {
            min: Vec3::new(-5000.0, 0.0, -5000.0),
            max: Vec3::new(5000.0, 5.0, 5000.0),
            floor_y: 0.0,
        };
        let grid = FloorGrid::new(&bounds);
        assert!(grid.cols * grid.rows <= FloorGrid::MAX_CELLS);
        assert!(grid.cell > FloorGrid::CELL_SIZE);
        // The grid still answers (fallback level everywhere).
        assert_eq!(grid.ground_height(0.0, 0.0, 5.0), 0.0);
    }

    #[test]
    fn per_cell_heights_are_capped() {
        // v0.2.2: one cell cannot accumulate unbounded heights.
        // The same 4x4 m floor triangle, added far more times than
        // the cap — every covered cell must stop at the cap.
        let mut grid = FloorGrid::new(&test_bounds());
        for _ in 0..(FloorGrid::MAX_HEIGHTS_PER_CELL + 100) {
            grid.add_triangle(
                Vec3::new(-2.0, 0.0, -2.0),
                Vec3::new(-2.0, 0.0, 2.0),
                Vec3::new(2.0, 0.0, 2.0),
            );
        }
        // Before finish(): no cell may hold more than the cap.
        // (finish() dedups near-identical heights, so the cap's
        // job is this pre-finish bound.)
        let mut covered = 0;
        for cell in &grid.heights {
            assert!(cell.len() <= FloorGrid::MAX_HEIGHTS_PER_CELL);
            if !cell.is_empty() {
                covered += 1;
            }
        }
        assert!(covered > 0);
        grid.finish();
        let (cells, _, _) = grid.stats();
        assert_eq!(cells, covered);
    }

    #[test]
    fn point_in_triangle_2d_basics() {
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(2.0, 0.0, 0.0);
        let c = Vec3::new(0.0, 0.0, 2.0);
        assert!(point_in_triangle_2d(0.5, 0.5, a, b, c));
        assert!(!point_in_triangle_2d(1.5, 1.5, a, b, c));
        assert!(!point_in_triangle_2d(-1.0, 0.5, a, b, c));
    }

    #[test]
    fn spawn_viewpoint_faces_out_of_the_inn_door() {
        // The inn's exterior XTEL arrival (real bytes, door_pair):
        // pos (23259.8, -44476.2, -8.3), yaw -0.842. The inn stands
        // EAST of the spawn and the Riverwood road is WEST, so the
        // arrival must face north-west — Bevy x negative (west)
        // and Bevy z negative (north, z = -Y). The old
        // counterclockwise formula faced north-east, into the
        // inn's west wall a metre away (M9 screenshot).
        let spawn = crate::cell::SpawnPoint {
            position: [23259.8, -44476.2, -8.3],
            yaw: -0.842,
        };
        let (eye, look_at) = spawn_viewpoint(&spawn);
        let dir = (look_at - eye).normalize();
        assert!(dir.x < -0.5, "must face west, got {dir}");
        assert!(dir.z < -0.5, "must face north, got {dir}");
        assert!(dir.y.abs() < 0.01, "must be level, got {dir}");
        // Eye is 1.7 m above the arrival point.
        assert!((eye.y - (-8.3 / 70.0 + crate::player::EYE_HEIGHT)).abs() < 0.001);
    }

    #[test]
    fn skyrim_falloff_is_linear_with_a_hard_cutoff() {
        // The game's formula: Fade x max(0, 1 - d/R). Inn light:
        // R = 512/70 = 7.314 m, Fade 1.0.
        let r = 512.0 / 70.0;
        assert_eq!(skyrim_falloff(0.0, r, 1.0), 1.0);
        // 1 m: 1 - 1/7.314 = 0.8633.
        assert!((skyrim_falloff(1.0, r, 1.0) - 0.8633).abs() < 0.001);
        // 2 m: 0.7266. 5 m: 0.3164. At the radius: 0. Past it: 0.
        assert!((skyrim_falloff(2.0, r, 1.0) - 0.7266).abs() < 0.001);
        assert!((skyrim_falloff(5.0, r, 1.0) - 0.3164).abs() < 0.001);
        assert_eq!(skyrim_falloff(r, r, 1.0), 0.0);
        assert_eq!(skyrim_falloff(r + 1.0, r, 1.0), 0.0);
        // Fade scales the whole curve: Fade 2.0 doubles every
        // point; Fade 0.5 halves it.
        assert!((skyrim_falloff(1.0, r, 2.0) - 2.0 * 0.8633).abs() < 0.001);
        assert!((skyrim_falloff(1.0, r, 0.5) - 0.5 * 0.8633).abs() < 0.001);
        // The shape difference that fixes the blowout: under the
        // old inverse-square law the 1 m / 5 m ratio was ~25x
        // (1/d^2); under the linear law it is 0.8633/0.3164 =
        // 2.73x — the near wall and the door live in the same
        // brightness world now.
        let ratio = skyrim_falloff(1.0, r, 1.0) / skyrim_falloff(5.0, r, 1.0);
        assert!((ratio - 2.73).abs() < 0.01, "1m/5m ratio {ratio}");
    }

    #[test]
    fn skyrim_light_intensity_is_fade_times_scale() {
        // Intensity = K x Fade, independent of radius: the
        // radius only sets the cutoff. K = 30,000 (render-
        // calibrated; see SKYRIM_LIGHT_LUX_SCALE — the value
        // set here is divided by 4pi in Bevy's light upload,
        // so the effective centre lux is K / 4pi).
        assert_eq!(skyrim_light_intensity(1.0), 30_000.0);
        assert_eq!(skyrim_light_intensity(2.0), 60_000.0);
        assert_eq!(skyrim_light_intensity(0.5), 15_000.0);
        // No fireplace special case: the game's data gives the
        // fireplace Fade 1.0, the same as every sconce.
        assert_eq!(skyrim_light_intensity(1.0), SKYRIM_LIGHT_LUX_SCALE);
    }

    #[test]
    fn shadow_budget_is_two_in_the_inn_and_capped_at_four() {
        // The inn's data: the fireplace + FarmInteriorShadowLight01
        // cast (both flags 0x1009), the sconces don't (0x9). Past
        // the cap, even a shadow-flagged light stays unshadowed.
        let fire = crate::cell::LightData {
            radius: 512,
            color: [0; 3],
            flags: 0x1009,
            falloff: 1.0,
            fade: 1.0,
        };
        let sconce = crate::cell::LightData { flags: 0x9, ..fire };
        assert!(light_casts_shadows(&fire, 0));
        assert!(light_casts_shadows(&fire, 1));
        assert!(!light_casts_shadows(&sconce, 0));
        assert!(!light_casts_shadows(&fire, MAX_SHADOW_LIGHTS_PER_SCENE));
        assert_eq!(MAX_SHADOW_LIGHTS_PER_SCENE, 4);
        // Phone budget: 512 texels per cubemap face, not Bevy's
        // 1024 default.
        assert_eq!(POINT_LIGHT_SHADOW_MAP_SIZE, 512);
    }

    #[test]
    fn interior_ambient_is_warm_bounce_not_a_wash() {
        // M16: 50 WHITE left Dylan's grey wash; 10 was the
        // bounce floor. v0.4.7: the v0.4.6 falloff fix made the
        // point lights ~12.6x brighter, so 10 now leaves black
        // corners, black table tops, and a black void for
        // Orgnar's face (lit only from above + ambient). 40
        // restores the bounce floor, and the WARM color (not
        // white) is what keeps it from washing grey — the fill
        // reads as firelight bounce. Render-proven.
        // v0.4.10: 40 was still too dark (Dylan: "the shadows
        // in the inn are still too dark") — ambient 40 sits
        // ~500x below the lit surfaces, so shadowed areas
        // render sRGB 25-40. 200 overshot (shadow 35 -> 77,
        // median 44 -> 85 — the warm wash); 100 is the
        // calibrated middle (shadow ~57, median ~66).
        // Render-proven on the before/after counter renders.
        assert_eq!(INTERIOR_AMBIENT_BRIGHTNESS, 100.0);
        assert_eq!(INTERIOR_AMBIENT_COLOR, Color::srgb(0.85, 0.62, 0.35));
    }

    #[test]
    fn exterior_ambient_is_the_sky_fill() {
        // Exterior follow-up: 800 left shaded pines near-black
        // (diagnostic: mean 0.233, 11% black); 2400 is the knee
        // (0.368, 0% black) and the clear-sky ratio — 20% of the
        // 12,000 lux sun.
        assert_eq!(EXTERIOR_AMBIENT_BRIGHTNESS, 2400.0);
    }

    #[test]
    fn exposure_is_indoor_inside_and_overcast_outside() {
        // M14: interiors use INDOOR (ev100 7.0), exteriors keep
        // OVERCAST (ev100 12.0) exactly as M11 left it.
        assert_eq!(exposure_for(false).ev100, 7.0);
        assert_eq!(exposure_for(true).ev100, 12.0);
    }

    #[test]
    fn spawn_progress_is_monotonic_and_reaches_one() {
        // The ANR fix: the loading screen's progress bar must
        // never go backwards, and must reach 100%. Drive a
        // progress through every phase boundary the way
        // `spawn_chunk` does (indexes only advance, phases only
        // advance) and assert each step is >= the last.
        let bounds = test_bounds();
        let mut progress = SpawnProgress::new(&bounds, HashSet::new(), true, 5, 3);
        let mut last = 0.0f32;
        assert_eq!(progress.progress(), 0.0);
        assert_eq!(progress.phase, SpawnPhase::Objects);
        for i in 0..=5 {
            progress.next_object = i;
            let p = progress.progress();
            assert!(p >= last, "progress went backwards: {last} -> {p}");
            assert!(p <= 1.0);
            last = p;
        }
        assert!((progress.progress() - 0.82).abs() < 0.001);
        progress.phase = SpawnPhase::Dressing;
        for i in 0..=3 {
            progress.next_land = i;
            let p = progress.progress();
            assert!(p >= last, "progress went backwards: {last} -> {p}");
            last = p;
        }
        assert!((progress.progress() - 0.90).abs() < 0.001);
        // No baker (map already built): MapBake reads as done.
        progress.phase = SpawnPhase::MapBake;
        assert!((progress.progress() - 0.99).abs() < 0.001);
        assert!(progress.progress() >= last);
        progress.phase = SpawnPhase::Finish;
        assert!((progress.progress() - 0.99).abs() < 0.001);
        assert!(progress.progress() >= last);
        progress.phase = SpawnPhase::Done;
        assert_eq!(progress.progress(), 1.0);
        assert_eq!(progress.progress_text(), "Done");
    }

    #[test]
    fn no_drawable_nifs_are_tracked_not_placeholdered() {
        // v0.4.7: a NIF that parsed but held no drawable
        // geometry (editor markers, particle-only) is
        // invisible in the retail game — its refs spawn
        // nothing, not a placeholder box. The regression that
        // proved the need: the editor-geometry filter turned
        // 35 exterior + 7 inn marker-only refs into coloured
        // placeholder boxes (exterior placeholders 5 -> 40).
        // `spawn_nif` records such NIFs in `no_drawable_cache`
        // and `spawn_object` skips them, counting each skipped
        // ref in `summary.mesh_no_drawable`. This pins the
        // wiring; the render proves the behaviour (exterior
        // placeholders back to 5, no_drawable 35).
        let bounds = test_bounds();
        let mut progress = SpawnProgress::new(&bounds, HashSet::new(), true, 5, 3);
        assert!(progress.no_drawable_cache.is_empty());
        assert_eq!(progress.summary.mesh_no_drawable, 0);
        assert_eq!(progress.summary.placeholder_objects, 0);
        progress
            .no_drawable_cache
            .insert("meshes\\effects\\ambient\\fxmiststreet01.nif".to_string());
        assert!(progress
            .no_drawable_cache
            .contains("meshes\\effects\\ambient\\fxmiststreet01.nif"));
        progress.summary.mesh_no_drawable += 1;
        assert_eq!(progress.summary.mesh_no_drawable, 1);
        assert_eq!(progress.summary.placeholder_objects, 0);
    }

    #[test]
    fn spawn_progress_map_bake_reports_baker_progress() {
        // The MapBake phase reads the baker: a fresh baker
        // reports 0.90 (the phase floor) + the baker's
        // "collecting cells" line; no baker (map already
        // built) reads as done at 0.99.
        let bounds = test_bounds();
        let mut progress = SpawnProgress::new(&bounds, HashSet::new(), true, 5, 3);
        progress.phase = SpawnPhase::MapBake;
        assert!((progress.progress() - 0.99).abs() < 0.001);
        assert_eq!(progress.progress_text(), "Drawing the map...");
        progress.map_baker = Some(crate::character::MapBaker::new());
        assert!((progress.progress() - 0.90).abs() < 0.001);
        assert_eq!(
            progress.progress_text(),
            "Drawing the map: collecting cells (0 groups)"
        );
    }

    #[test]
    fn spawn_progress_handles_an_empty_cell() {
        // A cell with no objects and no terrain (degenerate, but
        // the progress math must not divide by zero): Objects
        // reports 0.82, Dressing 0.90, MapBake (no baker) 0.99,
        // Done 1.0.
        let bounds = test_bounds();
        let mut progress = SpawnProgress::new(&bounds, HashSet::new(), true, 0, 0);
        assert!((progress.progress() - 0.82).abs() < 0.001);
        progress.phase = SpawnPhase::Dressing;
        assert!((progress.progress() - 0.90).abs() < 0.001);
        assert_eq!(progress.progress_text(), "Lighting the room...");
        progress.phase = SpawnPhase::MapBake;
        assert!((progress.progress() - 0.99).abs() < 0.001);
        progress.phase = SpawnPhase::Done;
        assert_eq!(progress.progress(), 1.0);
    }

    #[test]
    fn spawn_progress_text_counts_objects_and_terrain() {
        let bounds = test_bounds();
        let mut progress = SpawnProgress::new(&bounds, HashSet::new(), true, 437, 9);
        progress.next_object = 123;
        assert_eq!(progress.progress_text(), "Spawning objects 123/437");
        progress.phase = SpawnPhase::Dressing;
        progress.next_land = 4;
        assert_eq!(progress.progress_text(), "Building terrain 4/9");
        progress.phase = SpawnPhase::Finish;
        assert_eq!(progress.progress_text(), "Finishing...");
    }

    #[test]
    fn water_spawn_guard_defaults_to_unspawned() {
        // The Dressing-phase water race (forge milestone):
        // if the frame budget expires after `spawn_water`
        // but before the phase leaves Dressing, the next
        // frame re-runs Dressing and spawns water again —
        // render logs showed 12 (even 18) water cells vs
        // the correct 6. The `water_spawned` guard on the
        // progress keeps the planes spawning once per build.
        // This pins the default; the repeated exterior
        // renders prove the behaviour (water cells: 6
        // every time).
        let bounds = test_bounds();
        let mut progress = SpawnProgress::new(&bounds, HashSet::new(), true, 5, 3);
        assert!(!progress.water_spawned);
        progress.water_spawned = true;
        assert!(progress.water_spawned);
    }

    #[test]
    fn terrain_done_log_guard_defaults_to_unlogged() {
        // Same Dressing re-run as the water race: the
        // "terrain done" log sits after the terrain loop, so
        // a re-run frame (loop skipped, next_land already at
        // the end) fired it twice — proven on the after-ext
        // render (two "terrain done (9/9)" lines 134 ms
        // apart). The `terrain_done_logged` guard keeps the
        // boundary line firing once per build.
        let bounds = test_bounds();
        let mut progress = SpawnProgress::new(&bounds, HashSet::new(), true, 5, 3);
        assert!(!progress.terrain_done_logged);
        progress.terrain_done_logged = true;
        assert!(progress.terrain_done_logged);
    }

    #[test]
    fn spawn_chunk_budget_is_eight_milliseconds() {
        // The per-frame budget: ~8 ms of build work, checked
        // between objects — far under the 5 s ANR limit, with
        // room for a single expensive object to overrun it.
        assert_eq!(SPAWN_CHUNK_BUDGET, Duration::from_millis(8));
    }

    #[test]
    fn cached_geometry_survives_render_extraction() {
        // Regression test for the spawn_chunk panic (2026-11-15):
        // a mesh's vertex data is extracted to the RenderWorld
        // after the first frame (RENDER_WORLD usage), so the
        // cached path cannot re-read it from Assets. The
        // rasterization geometry is captured once at cache time
        // via mesh_geometry, and world_triangles works from the
        // captured copy alone — no Assets read at all.
        let mut mesh = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        );
        mesh.insert_indices(Indices::U32(vec![0, 1, 2]));
        let (positions, indices) = mesh_geometry(&mesh);
        assert_eq!(positions.len(), 3);
        assert_eq!(indices, vec![0, 1, 2]);
        // A translation moves the triangle with the ref.
        let transform =
            bevy::transform::components::Transform::from_translation(Vec3::new(10.0, 2.0, 0.0));
        let tris = world_triangles(&positions, &indices, &transform);
        assert_eq!(tris.len(), 1);
        assert_eq!(tris[0].0, Vec3::new(10.0, 2.0, 0.0));
        assert_eq!(tris[0].1, Vec3::new(11.0, 2.0, 0.0));
        assert_eq!(tris[0].2, Vec3::new(10.0, 2.0, 1.0));
        // Empty geometry rasterizes to nothing.
        assert!(world_triangles(&[], &[], &transform).is_empty());
    }
}
