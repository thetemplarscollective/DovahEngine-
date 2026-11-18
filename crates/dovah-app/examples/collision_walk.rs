//! Walk-sim proof for the per-object collision milestone.
//!
//! Builds the REAL FloorGrid + ObstacleGrid for Riverwood
//! exterior (5,-11) and the inn interior, mirroring the scene
//! build exactly (spawn_nif: collision-wins floor, obstacles
//! from collision except actors; trees: obstacles only, no
//! floor), then simulates `keyboard_move`'s rule step by step
//! (0.1 m steps: obstacle check, then ground probe at
//! feet + STEP_HEIGHT, ground = max(grid, terrain)).
//!
//! A. PORCH CLIMB (the milestone's acceptance test): street ->
//!    deck along the step_height_probe walk line. Must climb
//!    every tread (0.26 m rises) and reach the deck (-0.12 m).
//! B. FURNITURE (the other acceptance test): walk into a bench
//!    in the inn — must be BLOCKED by the obstacle grid and
//!    must NOT step onto the seat (ground stays at the floor).
//! C. WALL: walk into an inn wall — must be blocked.
//! D. TREE: walk into a pine trunk outdoors — must be blocked.
//! E. BRIDGE (regression): walk the bridge deck end to end —
//!    the feet must stay on the deck (it worked via visual
//!    triangles before; now it must work via collision).
//!
//! Usage: cargo run -p dovah-app --example collision_walk
//! (run from the workspace root; reads test-data/)

use bevy::math::Vec3;
use bevy::render::mesh::{Indices, Mesh, VertexAttributeValues};
use dovah_app::assets::AssetProvider;
use dovah_app::cell::{load_cell, load_exterior, CellData};
use dovah_app::collision::{
    collision_from_shapes, transform_footprint, CachedCollision, ObstacleGrid,
};
use dovah_app::coords;
use dovah_app::scene::{FloorGrid, RoomBounds, TerrainHeights};
use dovah_esm::Plugin;
use dovah_nif::NifFile;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const INN_EDID: &str = "RiverwoodSleepingGiantInn";
const STEP_HEIGHT: f32 = 0.3;
const PLAYER_HEIGHT: f32 = 1.7;
const STEP: f32 = 0.1;

/// Porch walk line (Bevy metres), from step_height_probe's
/// offset-0.0 profile: A = deck/spawn, B = street.
const WALK_A: (f32, f32) = (332.28, 635.37);
const WALK_B: (f32, f32) = (328.04, 631.56);

/// Pull positions + indices out of a converted Bevy mesh
/// (scene.rs `mesh_geometry`, replicated — it is private).
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

fn world_triangles(
    positions: &[[f32; 3]],
    indices: &[u32],
    matrix: &bevy::math::Mat4,
) -> Vec<(Vec3, Vec3, Vec3)> {
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

/// Build both grids for a loaded cell, mirroring `spawn_nif` +
/// `spawn_object` exactly. Returns (floor, obstacle, NIFs with
/// collision, refs rasterized).
fn build_grids(
    provider: &mut AssetProvider,
    data: &CellData,
    bounds: &RoomBounds,
) -> (FloorGrid, ObstacleGrid, usize, usize) {
    let mut floor = FloorGrid::new(bounds);
    let mut obstacles = ObstacleGrid::new(bounds);
    let mut collision_cache: HashMap<String, Option<Arc<CachedCollision>>> = HashMap::new();
    let mut nifs_with_collision = 0usize;
    let mut refs = 0usize;
    for object in &data.objects {
        if object.is_actor {
            continue;
        }
        let Some(model_path) = &object.model_path else {
            continue;
        };
        let rasterize = !matches!(object.base_type.as_str(), "TREE" | "FLOR" | "GRAS");
        let nif_path = format!("meshes\\{model_path}");
        let collision = match collision_cache.get(&nif_path) {
            Some(c) => c.clone(),
            None => {
                let c = provider
                    .read_file(&nif_path)
                    .and_then(|bytes| NifFile::from_bytes(&bytes).ok())
                    .and_then(|nif| collision_from_shapes(&nif.collision_shapes()))
                    .map(Arc::new);
                if c.is_some() {
                    nifs_with_collision += 1;
                }
                collision_cache.insert(nif_path.clone(), c.clone());
                c
            }
        };
        let position = coords::to_bevy_position(object.position);
        let rotation = coords::to_bevy_rotation(object.rotation);
        let transform = bevy::transform::components::Transform {
            translation: position,
            rotation,
            scale: Vec3::splat(object.scale),
        };
        let matrix = transform.to_matrix();
        // Collision rasterization (mirrors rasterize_collision).
        let mut collision_floor = false;
        if let Some(collision) = &collision {
            if rasterize && collision.has_mesh() {
                collision_floor = true;
                for (a, b, c) in
                    world_triangles(&collision.positions, &collision.floor_indices, &matrix)
                {
                    floor.add_triangle(a, b, c);
                }
            }
            if collision.has_obstacles() {
                for (a, b, c) in
                    world_triangles(&collision.positions, &collision.obstacle_indices, &matrix)
                {
                    obstacles.add_triangle(a, b, c);
                }
                for footprint in collision.footprints.iter() {
                    let (min_x, max_x, min_z, max_z, min_y, max_y) =
                        transform_footprint(footprint, &transform);
                    obstacles.add_footprint(min_x, max_x, min_z, max_z, min_y, max_y);
                }
            }
        }
        // Visual floor rasterization (skipped when the floor
        // came from collision).
        if rasterize && !collision_floor {
            let Some(bytes) = provider.read_file(&nif_path) else {
                continue;
            };
            let Ok(nif) = NifFile::from_bytes(&bytes) else {
                continue;
            };
            for sub in nif.meshes() {
                let Some(mesh) = dovah_app::convert::nif_mesh_to_bevy(&sub) else {
                    continue;
                };
                let (positions, indices) = mesh_geometry(&mesh);
                for (a, b, c) in world_triangles(&positions, &indices, &matrix) {
                    floor.add_triangle(a, b, c);
                }
            }
        }
        refs += 1;
    }
    floor.finish();
    obstacles.finish(&floor, STEP_HEIGHT);
    (floor, obstacles, nifs_with_collision, refs)
}

/// One `keyboard_move` step: horizontal wish of `STEP` along
/// `dir` from `feet`, then the ground probe, then the obstacle
/// check from the NEW feet (grounded band). Returns (new feet,
/// blocked?). The vertical step teleports to the ground (the
/// step_height_probe convention — the probe's steps are small
/// and its walks have no ledge drops; the real mover falls
/// gradually under gravity, which matters only for drops).
fn walk_step(
    floor: &FloorGrid,
    obstacles: &ObstacleGrid,
    terrain: Option<&TerrainHeights>,
    feet: Vec3,
    dir: Vec3,
) -> (Vec3, bool) {
    let next_x = feet.x + dir.x * STEP;
    let next_z = feet.z + dir.z * STEP;
    // Vertical first (mirrors keyboard_move): the ground probe
    // is feet + STEP_HEIGHT (grounded walking).
    let probe = feet.y + STEP_HEIGHT;
    let mut ground = floor.ground_height(next_x, next_z, probe);
    if let Some(terrain) = terrain {
        if let Some(th) = terrain.height_at(next_x, next_z) {
            ground = ground.max(th);
        }
    }
    // Obstacle check from the NEW feet: a stair riser tops out
    // below the band from the tread above it.
    if obstacles.blocked(next_x, next_z, ground + STEP_HEIGHT, ground + PLAYER_HEIGHT) {
        return (feet, true);
    }
    (Vec3::new(next_x, ground, next_z), false)
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut provider = AssetProvider::new(Path::new("test-data"));
    println!(
        "provider indexed {} files from {} archives",
        provider.indexed_files(),
        provider.archive_names.len()
    );

    // ---- Exterior grids ----
    let exterior = load_exterior(&plugin, (5, -11), None).expect("load Riverwood");
    println!("\nexterior: {} objects", exterior.objects.len());
    let floor_y = exterior
        .lands
        .iter()
        .find(|l| l.grid == (5, -11))
        .and_then(|l| l.sample_height(2048.0, 2048.0))
        .map(|h| h / coords::SKYRIM_UNITS_PER_METER)
        .unwrap_or(0.0);
    let ext_bounds = RoomBounds::from_exterior((5, -11), floor_y);
    let (ext_floor, ext_obstacles, ext_nifs, ext_refs) =
        build_grids(&mut provider, &exterior, &ext_bounds);
    let (fcells, fmin, fmax) = ext_floor.stats();
    let (ocells, ointervals) = ext_obstacles.stats();
    println!(
        "  floor grid: {fcells} cells, heights {fmin:.2}..{fmax:.2} m; obstacle grid: {ocells} cells, {ointervals} intervals; {ext_nifs} NIFs with collision, {ext_refs} refs rasterized"
    );
    let terrain = TerrainHeights::new(&exterior.lands);

    // ---- A. porch climb: street -> deck ----
    println!("\n=== A. PORCH CLIMB (street -> deck) ===");
    let dir = Vec3::new(WALK_A.0 - WALK_B.0, 0.0, WALK_A.1 - WALK_B.1).normalize();
    let start_terrain = terrain.height_at(WALK_B.0, WALK_B.1).unwrap_or(floor_y);
    let mut feet = Vec3::new(WALK_B.0, start_terrain, WALK_B.1);
    println!(
        "  start ({:.2}, {:.2}) feet {:.2} m (terrain), walking {:.2} m to the deck",
        feet.x,
        feet.z,
        feet.y,
        ((WALK_A.0 - WALK_B.0).powi(2) + (WALK_A.1 - WALK_B.1).powi(2)).sqrt()
    );
    let mut max_rise = 0.0f32;
    let mut blocked_at: Option<(f32, f32, f32)> = None;
    let mut d = 0.0f32;
    let total = ((WALK_A.0 - WALK_B.0).powi(2) + (WALK_A.1 - WALK_B.1).powi(2)).sqrt();
    while d < total {
        let prev_y = feet.y;
        let (next, blocked) = walk_step(&ext_floor, &ext_obstacles, Some(&terrain), feet, dir);
        if blocked {
            blocked_at = Some((next.x, next.z, next.y));
            break;
        }
        let rise = next.y - prev_y;
        if rise > 0.005 {
            max_rise = max_rise.max(rise);
            println!(
                "    d={d:.1} m  ({:.2}, {:.2})  feet {prev_y:.2} -> {next_y:.2} m  rise +{rise:.2} m",
                next.x,
                next.z,
                next_y = next.y
            );
        }
        feet = next;
        d += STEP;
    }
    match blocked_at {
        Some((x, z, y)) => {
            let intervals = ext_obstacles.intervals_at(x + dir.x * STEP, z + dir.z * STEP);
            let ground =
                ext_floor.ground_height(x + dir.x * STEP, z + dir.z * STEP, y + STEP_HEIGHT);
            println!("  BLOCKED at ({x:.2}, {z:.2}) feet {y:.2} m — climb FAILED");
            println!(
                "    ground ahead {ground:.2} m, band [{:.2}, {:.2}], intervals ahead: {intervals:?}",
                ground + STEP_HEIGHT,
                ground + PLAYER_HEIGHT
            );
        }
        None => println!(
            "  reached the deck: feet {:.2} m (deck is -0.12 m), max single rise {:.2} m — climb OK",
            feet.y, max_rise
        ),
    }

    // ---- D. pine trunk ----
    println!("\n=== D. PINE TRUNK ===");
    let mut trunk_tested = false;
    for object in &exterior.objects {
        if object.base_type != "TREE" || !object.base_edid.contains("PineForest") {
            continue;
        }
        let pos = coords::to_bevy_position(object.position);
        // Walk toward the trunk from 3 m away (from the south:
        // -Z is open road for most Riverwood pines; verify the
        // start cell is open first).
        let start = Vec3::new(pos.x, 0.0, pos.z + 3.0);
        let start_ground = ext_floor.ground_height(start.x, start.z, 10.0).max(
            terrain
                .height_at(start.x, start.z)
                .unwrap_or(f32::NEG_INFINITY),
        );
        let start = Vec3::new(start.x, start_ground, start.z);
        if ext_obstacles.blocked(
            start.x,
            start.z,
            start.y + STEP_HEIGHT,
            start.y + PLAYER_HEIGHT,
        ) {
            continue;
        }
        let dir = Vec3::new(0.0, 0.0, -1.0);
        let mut feet = start;
        let mut blocked = false;
        let mut steps = 0;
        while steps < 40 {
            let (next, b) = walk_step(&ext_floor, &ext_obstacles, Some(&terrain), feet, dir);
            if b {
                blocked = true;
                break;
            }
            feet = next;
            steps += 1;
        }
        let dist = ((feet.x - pos.x).powi(2) + (feet.z - pos.z).powi(2)).sqrt();
        println!(
            "  {} at ({:.1}, {:.1}): walked {} steps, stopped {:.2} m from the trunk centre, blocked={blocked}",
            object.base_edid, pos.x, pos.z, steps, dist
        );
        trunk_tested = true;
        break;
    }
    if !trunk_tested {
        println!("  (no suitable pine found)");
    }

    // ---- E. bridge ----
    println!("\n=== E. BRIDGE (regression) ===");
    let mut bridge_tested = false;
    for object in &exterior.objects {
        if !object.base_edid.contains("Bridge01") {
            continue;
        }
        let pos = coords::to_bevy_position(object.position);
        // The bridge's long axis is its local Y (AABB y ±21 m vs
        // x ±4.6 m). World direction of local +Y: rotate (0,1,0)
        // by the ref rotation, then to Bevy.
        let rot = coords::to_bevy_rotation(object.rotation);
        let axis = rot * Vec3::new(0.0, 0.0, -1.0); // local +Y -> Bevy -Z
        let axis = Vec3::new(axis.x, 0.0, axis.z).normalize();
        // Start 18 m from the centre along -axis, at the deck.
        let start = pos - axis * 18.0;
        let start_ground = ext_floor.ground_height(start.x, start.z, 10.0);
        let mut feet = Vec3::new(start.x, start_ground, start.z);
        println!(
            "  bridge at ({:.1}, {:.1}), axis ({:.2}, {:.2}), start feet {:.2} m",
            pos.x, pos.z, axis.x, axis.z, feet.y
        );
        let mut min_y = feet.y;
        let mut max_y = feet.y;
        let mut blocked = false;
        let mut steps = 0;
        while steps < 400 {
            let (next, b) = walk_step(&ext_floor, &ext_obstacles, Some(&terrain), feet, axis);
            if b {
                blocked = true;
                break;
            }
            feet = next;
            min_y = min_y.min(feet.y);
            max_y = max_y.max(feet.y);
            steps += 1;
            if (feet - pos).length() > 20.0 && steps > 200 {
                break;
            }
        }
        println!(
            "  walked {} steps ({:.1} m), feet range {:.2}..{:.2} m, blocked={blocked}, end ({:.1}, {:.1})",
            steps,
            steps as f32 * STEP,
            min_y,
            max_y,
            feet.x,
            feet.z
        );
        bridge_tested = true;
        break;
    }
    if !bridge_tested {
        println!("  (no Bridge01 ref found)");
    }

    // ---- Interior grids ----
    let interior = load_cell(&plugin, INN_EDID, None).expect("load inn interior");
    println!("\ninn interior: {} objects", interior.objects.len());
    let inn_bounds = RoomBounds::from_cell(&interior);
    let (inn_floor, inn_obstacles, inn_nifs, inn_refs) =
        build_grids(&mut provider, &interior, &inn_bounds);
    let (fcells, fmin, fmax) = inn_floor.stats();
    let (ocells, ointervals) = inn_obstacles.stats();
    println!(
        "  floor grid: {fcells} cells, heights {fmin:.2}..{fmax:.2} m; obstacle grid: {ocells} cells, {ointervals} intervals; {inn_nifs} NIFs with collision, {inn_refs} refs rasterized"
    );
    println!(
        "  bounds x {:.1}..{:.1} z {:.1}..{:.1} floor_y {:.2} m",
        inn_bounds.min.x, inn_bounds.max.x, inn_bounds.min.z, inn_bounds.max.z, inn_bounds.floor_y
    );

    // ---- B. furniture: every bench blocks, none is steppable ----
    println!("\n=== B. BENCHES (must block; seats must not be steppable) ===");
    let mut benches = 0usize;
    let mut walked = false;
    for object in &interior.objects {
        if !object.base_edid.contains("Bench") {
            continue;
        }
        benches += 1;
        let pos = coords::to_bevy_position(object.position);
        // The bench stands on its ref origin's height (NIF
        // origins sit at the base). The seat is 0.43 m above
        // it (collision_design Part D).
        let local_floor = pos.y;
        // The seat IS in the floor grid (probe floor+0.5 in a
        // 1 m radius) but ABOVE the step probe (floor+0.3):
        // the player must not step onto it.
        let mut seat_top = f32::NEG_INFINITY;
        let mut dx = -1.0f32;
        while dx <= 1.0 {
            let mut dz = -1.0f32;
            while dz <= 1.0 {
                let h = inn_floor.ground_height(pos.x + dx, pos.z + dz, local_floor + 0.5);
                seat_top = seat_top.max(h);
                dz += 0.25;
            }
            dx += 0.25;
        }
        let blocked_at_origin = inn_obstacles.blocked(
            pos.x,
            pos.z,
            local_floor + STEP_HEIGHT,
            local_floor + PLAYER_HEIGHT,
        );
        let ground_at_origin = inn_floor.ground_height(pos.x, pos.z, local_floor + STEP_HEIGHT);
        println!(
            "  {} at ({:.2}, {:.2}, {:.2}): floor {:.2} m, seat top within 1 m {:.2} m (expect floor+0.43), blocked at origin={blocked_at_origin}, ground at origin with step probe {:.2} m (expect the floor)",
            object.base_edid, pos.x, pos.z, pos.y, local_floor, seat_top, ground_at_origin
        );
        // Walk into the first bench with an open approach.
        if walked {
            continue;
        }
        for dir in [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, -1.0),
        ] {
            let start = Vec3::new(pos.x - dir.x * 2.0, local_floor, pos.z - dir.z * 2.0);
            if inn_obstacles.blocked(
                start.x,
                start.z,
                start.y + STEP_HEIGHT,
                start.y + PLAYER_HEIGHT,
            ) {
                continue;
            }
            let mut feet = start;
            let mut blocked = false;
            let mut steps = 0;
            let mut max_feet = feet.y;
            while steps < 40 {
                let (next, b) = walk_step(&inn_floor, &inn_obstacles, None, feet, dir);
                if b {
                    blocked = true;
                    break;
                }
                feet = next;
                max_feet = max_feet.max(feet.y);
                steps += 1;
            }
            let dist = ((feet.x - pos.x).powi(2) + (feet.z - pos.z).powi(2)).sqrt();
            let intervals =
                inn_obstacles.intervals_at(feet.x + dir.x * STEP, feet.z + dir.z * STEP);
            println!(
                "  WALK approach dir ({:.0}, {:.0}): walked {} steps, stopped {:.2} m from the bench origin, max feet {:.2} m (floor {:.2}), blocked={blocked}, intervals ahead: {:?}",
                dir.x, dir.z, steps, dist, max_feet, local_floor, intervals
            );
            walked = true;
            break;
        }
    }
    println!("  benches checked: {benches}, walk done: {walked}");

    // ---- C. walls: walk in 4 directions from an open point ----
    println!("\n=== C. WALLS (walk in 4 directions from an open point) ===");
    // Find an open start point near the taproom centre: scan a
    // lattice for a cell whose ground is the room floor (not a
    // furniture top) and that is not blocked. (The bounds centre
    // sits on a table.)
    let mut start: Option<Vec3> = None;
    let mut x = -5.0f32;
    'scan: while x < 5.0 {
        let mut z = -10.0f32;
        while z < 5.0 {
            let g = inn_floor.ground_height(x, z, inn_bounds.floor_y + STEP_HEIGHT);
            if (g - inn_bounds.floor_y).abs() < 0.05
                && !inn_obstacles.blocked(x, z, g + STEP_HEIGHT, g + PLAYER_HEIGHT)
            {
                start = Some(Vec3::new(x, g, z));
                break 'scan;
            }
            z += 0.5;
        }
        x += 0.5;
    }
    let Some(start) = start else {
        println!("  (no open start point found)");
        println!("\nprobe done.");
        return;
    };
    println!(
        "  start ({:.2}, {:.2}) feet {:.2} m",
        start.x, start.z, start.y
    );
    for (name, dir) in [
        ("+X (east)", Vec3::new(1.0, 0.0, 0.0)),
        ("-X (west)", Vec3::new(-1.0, 0.0, 0.0)),
        ("+Z (north)", Vec3::new(0.0, 0.0, 1.0)),
        ("-Z (south)", Vec3::new(0.0, 0.0, -1.0)),
    ] {
        let mut feet = start;
        let mut blocked = false;
        let mut steps = 0;
        while steps < 600 {
            let (next, b) = walk_step(&inn_floor, &inn_obstacles, None, feet, dir);
            if b {
                blocked = true;
                break;
            }
            feet = next;
            steps += 1;
            if feet.x > inn_bounds.max.x - 0.5
                || feet.x < inn_bounds.min.x + 0.5
                || feet.z > inn_bounds.max.z - 0.5
                || feet.z < inn_bounds.min.z + 0.5
            {
                break;
            }
        }
        let intervals = inn_obstacles.intervals_at(feet.x + dir.x * STEP, feet.z + dir.z * STEP);
        println!(
            "  {name}: walked {} steps ({:.1} m), stopped at ({:.2}, {:.2}), blocked={blocked}, intervals ahead: {:?}",
            steps,
            steps as f32 * STEP,
            feet.x,
            feet.z,
            intervals
        );
    }

    println!("\nprobe done.");
}
