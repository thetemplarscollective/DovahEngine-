//! Measurement probe for the inn-return fix: what does the
//! FloorGrid actually say?
//!
//! Part A (exterior Riverwood 5,-11): the ground height at the
//! XTEL arrival spawn, plus a height profile walking from the
//! porch deck down the steps to the street — the porch rises
//! decide the new STEP_HEIGHT.
//! Part B (inn interior): every distinct floor height in the
//! inn, to find the lowest furniture top — the STEP_HEIGHT
//! ceiling (the player must not step onto chairs).
//!
//! Builds the REAL FloorGrid (scene.rs) from the scene's meshes,
//! mirroring spawn_object: actors skipped, TREE/FLOR/GRAS never
//! rasterized, model-less objects skipped.
//!
//! Usage: cargo run -p dovah-app --example step_height_probe

use bevy::math::Vec3;
use bevy::render::mesh::{Indices, Mesh, VertexAttributeValues};
use dovah_app::assets::AssetProvider;
use dovah_app::cell::{load_cell, load_exterior, CellData};
use dovah_app::coords;
use dovah_app::scene::{FloorGrid, RoomBounds, TerrainHeights};
use dovah_esm::Plugin;
use dovah_nif::NifFile;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const INN_EDID: &str = "RiverwoodSleepingGiantInn";
/// XTEL arrival spawn, Bevy metres (inn_door_probe2.log Part A).
const SPAWN: Vec3 = Vec3::new(332.28, -0.12, 635.37);
/// Doorway plane normal: spawn -> door, INTO the inn. The
/// street is the opposite direction (-OUT_NORMAL).
const OUT_NORMAL: Vec3 = Vec3::new(0.743, 0.0, 0.669);
/// Exterior inn door trigger, Bevy metres.
const DOOR_XZ: (f32, f32) = (333.15, 636.15);

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

/// Build the FloorGrid for a loaded cell exactly as the scene
/// build does. `band` collects (EDID, triangle count, min Y,
/// max Y) for objects with up-facing triangles in the absolute
/// Y band (lo, hi) — the furniture attribution for Part B.
fn build_grid(
    provider: &mut AssetProvider,
    data: &CellData,
    bounds: &RoomBounds,
    band: (f32, f32),
    band_hits: &mut HashMap<String, (u64, f32, f32)>,
) -> FloorGrid {
    let mut grid = FloorGrid::new(bounds);
    let mut objects = 0u64;
    let mut tris = 0u64;
    for object in &data.objects {
        if object.is_actor {
            continue;
        }
        if matches!(object.base_type.as_str(), "TREE" | "FLOR" | "GRAS") {
            continue;
        }
        let Some(model_path) = &object.model_path else {
            continue;
        };
        let nif_path = format!("meshes\\{model_path}");
        let Some(bytes) = provider.read_file(&nif_path) else {
            continue;
        };
        let Ok(nif) = NifFile::from_bytes(&bytes) else {
            continue;
        };
        objects += 1;
        let position = coords::to_bevy_position(object.position);
        let rotation = coords::to_bevy_rotation(object.rotation);
        let transform = bevy::transform::components::Transform {
            translation: position,
            rotation,
            scale: Vec3::splat(object.scale),
        };
        let matrix = transform.to_matrix();
        for sub in nif.meshes() {
            let Some(mesh) = dovah_app::convert::nif_mesh_to_bevy(&sub) else {
                continue;
            };
            let (positions, indices) = mesh_geometry(&mesh);
            let world: Vec<Vec3> = positions
                .iter()
                .map(|p| matrix.transform_point3(Vec3::from(*p)))
                .collect();
            for t in indices.as_chunks::<3>().0 {
                let (Some(&a), Some(&b), Some(&c)) = (
                    world.get(t[0] as usize),
                    world.get(t[1] as usize),
                    world.get(t[2] as usize),
                ) else {
                    continue;
                };
                tris += 1;
                grid.add_triangle(a, b, c);
                // Furniture attribution: up-facing triangles in
                // the band (same filter as add_triangle).
                let normal = (b - a).cross(c - a);
                if normal.y <= 0.0 {
                    continue;
                }
                let n = normal.normalize();
                if n.y < 0.5 {
                    continue;
                }
                let y = a.y.min(b.y).min(c.y);
                if y > band.0 && y < band.1 {
                    let entry = band_hits.entry(object.base_edid.clone()).or_insert((
                        0,
                        f32::INFINITY,
                        f32::NEG_INFINITY,
                    ));
                    entry.0 += 1;
                    entry.1 = entry.1.min(a.y.min(b.y).min(c.y));
                    entry.2 = entry.2.max(a.y.max(b.y).max(c.y));
                }
            }
        }
    }
    grid.finish();
    println!("  rasterized {objects} objects, {tris} triangles");
    grid
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut provider = AssetProvider::new(Path::new("test-data"));
    println!(
        "provider indexed {} files from {} archives",
        provider.indexed_files(),
        provider.archive_names.len()
    );

    // ---- Part A: exterior porch ----
    println!("\n=== Part A: exterior Riverwood (5,-11), inn porch ===");
    let exterior = load_exterior(&plugin, (5, -11), None).expect("load Riverwood");
    println!("  {} objects", exterior.objects.len());
    let floor_y = exterior
        .lands
        .iter()
        .find(|l| l.grid == (5, -11))
        .and_then(|l| l.sample_height(2048.0, 2048.0))
        .map(|h| h / coords::SKYRIM_UNITS_PER_METER)
        .unwrap_or(0.0);
    let bounds = RoomBounds::from_exterior((5, -11), floor_y);
    println!("  exterior fallback floor_y: {floor_y:.2} m");
    let mut band_hits = HashMap::new();
    let grid = build_grid(
        &mut provider,
        &exterior,
        &bounds,
        (f32::NEG_INFINITY, f32::NEG_INFINITY),
        &mut band_hits,
    );
    let (cells, min_h, max_h) = grid.stats();
    println!("  grid: {cells} cells with data, heights {min_h:.2}..{max_h:.2} m");
    // The player's ground probe is feet + STEP_HEIGHT (player.rs):
    // the highest floor at or below the probe. Feet start at the
    // spawn Y (-0.12 m, the porch deck). Query with the CURRENT
    // step height (0.3) and the candidate (0.5) — both must read
    // the deck (-0.12), not the fallback (-3.89) or the roof.
    for step in [0.3f32, 0.5] {
        let probe = SPAWN.y + step;
        let spawn_ground = grid.ground_height(SPAWN.x, SPAWN.z, probe);
        let door_ground = grid.ground_height(DOOR_XZ.0, DOOR_XZ.1, probe);
        println!(
            "  STEP_HEIGHT {step}: ground at spawn {spawn_ground:.2} m, at door trigger {door_ground:.2} m"
        );
    }
    // Profile: walk from the spawn toward the STREET (-OUT_NORMAL;
    // the door normal points spawn -> door, INTO the inn). Feet
    // track the walk: step up when the next ground is within
    // STEP_HEIGHT, drop down otherwise — the player.rs rule.
    // Ground is max(grid, terrain): the grid alone misses the
    // street itself, so both are printed. Three parallel lines
    // (0.1 m steps): the centre line may cross the porch edge
    // where there is no step, while the real steps sit to one
    // side — the walk UP is the reverse of the drops below.
    println!("  --- profile: spawn -> street (0.1 m steps, feet tracked) ---");
    let terrain = TerrainHeights::new(&exterior.lands);
    let perp = Vec3::new(-OUT_NORMAL.z, 0.0, OUT_NORMAL.x);
    for offset in [-1.0f32, 0.0, 1.0] {
        println!("  line offset {offset:+.1} m:");
        let start = SPAWN + perp * offset;
        let mut feet = SPAWN.y;
        let mut max_drop: f32 = 0.0;
        let mut d = 0.0f32;
        while d <= 6.0 {
            let p = start - OUT_NORMAL * d;
            let grid_g = grid.ground_height(p.x, p.z, feet + 0.3);
            let terr = terrain.height_at(p.x, p.z);
            let ground = terr.map_or(grid_g, |t| grid_g.max(t));
            let delta = ground - feet;
            if delta < -0.005 {
                max_drop = max_drop.max(-delta);
            }
            // Print only the changes (the flat runs are noise).
            if (delta.abs() > 0.005) || d == 0.0 {
                let terr_s = terr.map_or("  --  ".to_string(), |t| format!("{t:5.2}"));
                println!(
                    "    d={d:.1} m  ({:.2}, {:.2})  grid {grid_g:5.2} m  terr {terr_s} m  ground {ground:5.2} m  feet {feet:5.2} m  delta {delta:+.2} m",
                    p.x, p.z
                );
            }
            feet = ground;
            d += 0.1;
        }
        println!("    max single drop (== max rise walking up): {max_drop:.2} m");
    }

    // ---- Part B: inn interior heights ----
    println!("\n=== Part B: inn interior distinct floor heights ===");
    let interior = load_cell(&plugin, INN_EDID, None).expect("load inn interior");
    println!("  {} objects", interior.objects.len());
    let bounds = RoomBounds::from_cell(&interior);
    println!(
        "  bounds x {:.1}..{:.1}  z {:.1}..{:.1}  floor_y {:.2} m",
        bounds.min.x, bounds.max.x, bounds.min.z, bounds.max.z, bounds.floor_y
    );
    let mut band_hits = HashMap::new();
    let grid = build_grid(
        &mut provider,
        &interior,
        &bounds,
        (0.25, 0.65),
        &mut band_hits,
    );
    let (cells, min_h, max_h) = grid.stats();
    println!("  grid: {cells} cells with data, heights {min_h:.2}..{max_h:.2} m");
    // Distinct heights by sampling the grid on a 0.25 m lattice,
    // probed from the inn floor (feet 0.0) with max_y 0.65 m —
    // every surface in step range. Surfaces above 0.65 (tables,
    // the upper floor) are out of step range whatever the value.
    let mut counts: BTreeMap<i32, u64> = BTreeMap::new();
    let mut x = bounds.min.x;
    while x <= bounds.max.x {
        let mut z = bounds.min.z;
        while z <= bounds.max.z {
            let h = grid.ground_height(x, z, 0.65);
            *counts.entry((h * 100.0).round() as i32).or_insert(0) += 1;
            z += 0.25;
        }
        x += 0.25;
    }
    println!("  --- distinct heights (cm, samples) ---");
    for (cm, n) in &counts {
        println!("    {:>6.2} m  {:>5} samples", *cm as f32 / 100.0, n);
    }
    println!("  --- objects with up-facing triangles in Y (0.25, 0.65) ---");
    let mut hits: Vec<(&String, &(u64, f32, f32))> = band_hits.iter().collect();
    hits.sort_by_key(|(_, (n, _, _))| std::cmp::Reverse(*n));
    for (edid, (n, lo, hi)) in hits.iter().take(25) {
        println!("    {edid}: {n} tris, Y {lo:.2}..{hi:.2} m");
    }
    println!("\nprobe done.");
}
