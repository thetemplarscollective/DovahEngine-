//! v0.4.6 phone-feedback probe: why the player cannot get BACK
//! into the Sleeping Giant Inn, and what the big yellow/green
//! placeholder boxes outdoors actually are.
//!
//! Part A: the interior inn door (the direction that WORKS — the
//! control): its ref, XTEL, resolved target, model, hinge data.
//! Part B: the exterior inn door (the direction that FAILS):
//! same trace. The exterior door ref is identified by the
//! interior door's XTEL dest_ref — the two doors point at each
//! other. Every exterior door trigger is printed, with its
//! resolved target, so a wrong resolution stands out.
//! Part C: XTEL target resolution, replicated from cell.rs:
//! parent_cell_of(dest_ref) -> parent CELL record -> interior?
//! -> Interior{edid}, else Exterior{grid_of_position(pos)}.
//! Part D: the door NIFs — hinge nodes + Open sequences, the
//! two things a DoorRuntime needs (spawn_nif only records a
//! swinging door when a hinge-node sub-mesh converts).
//! Part E: placeholder census for the exterior: every object
//! whose model file is missing from the Meshes BSAs, with OBND
//! size, ref scale, SCALED size (placeholders inherit the ref's
//! scale — scene.rs spawns the box under the object's full
//! transform), and Bevy position. Sorted by scaled volume.
//! Part F: geometry simulation — walk from the exterior arrival
//! spawn through the exterior doorway and test the exact
//! predicate the game uses (`swinging_door_fires` from doors.rs
//! — the probe's old local copy of the crossing test drifted,
//! so Part F now builds a real DoorRuntime) at porch height and
//! at terrain height.
//!
//! Usage: cargo run -p dovah-app --example inn_door_probe

use bevy::math::{Mat3, Quat, Vec3};
use bevy::transform::components::Transform;
use dovah_app::assets::AssetProvider;
use dovah_app::cell::{
    find_xtel_spawn, grid_of_position, load_cell, load_exterior, CellIndex, SceneTarget,
};
use dovah_app::doors::{swinging_door_fires, DoorAnim, DoorRuntime};
use dovah_bsa::BsaArchive;
use dovah_esm::{Plugin, Record};
use dovah_nif::NifFile;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const INN_EDID: &str = "RiverwoodSleepingGiantInn";

/// Skyrim units per metre.
const UPM: f32 = 70.0;

/// Skyrim [x, y, z] units -> Bevy metres (coords.rs).
fn to_bevy(p: [f32; 3]) -> Vec3 {
    Vec3::new(p[0] / UPM, p[2] / UPM, -p[1] / UPM)
}

/// Skyrim Euler [rx, ry, rz] -> Bevy quat (coords.rs: yaw negated).
fn to_bevy_rot(r: [f32; 3]) -> Quat {
    let m = Mat3::from_cols(Vec3::X, Vec3::new(0.0, 0.0, -1.0), Vec3::Y);
    let sky =
        Mat3::from_rotation_z(-r[2]) * Mat3::from_rotation_y(r[1]) * Mat3::from_rotation_x(r[0]);
    Quat::from_mat3(&(m * sky * m.transpose()))
}

/// CELL DATA first byte bit 0 (cell.rs).
fn cell_is_interior(cell: &Record) -> bool {
    cell.field(b"DATA")
        .and_then(|f| f.data.first())
        .map(|b| b & 0x01 != 0)
        .unwrap_or(false)
}

/// Hinge-node rule, copied from doors.rs.
fn is_hinge_node(name: &str) -> bool {
    for base in ["Door", "LeftDoor", "RightDoor"] {
        if name == base {
            return true;
        }
        if name.len() > base.len()
            && name.starts_with(base)
            && name[base.len()..].chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

/// Open angle + duration from a NIF's Open sequence, copied from
/// doors.rs `open_angle_from_nif`.
fn open_angle_from_nif(nif: &NifFile) -> Option<(f32, f32)> {
    let at_rest = |v: f32| {
        let wrapped = v.rem_euclid(2.0 * std::f32::consts::PI);
        wrapped < 1e-3 || wrapped > 2.0 * std::f32::consts::PI - 1e-3
    };
    let mut best: Option<(f32, f32)> = None;
    for data in nif.transform_data() {
        let Some(first) = data.z_keys.first() else {
            continue;
        };
        let Some(last) = data.z_keys.last() else {
            continue;
        };
        if !at_rest(first.value) || at_rest(last.value) {
            continue;
        }
        let candidate = (last.value - first.value, last.time);
        let better = match best {
            None => true,
            Some((angle, _)) => candidate.0.abs() > angle.abs(),
        };
        if better {
            best = Some(candidate);
        }
    }
    best
}

/// The doorway-crossing predicate, copied from doors.rs.
fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let all = plugin.all_records();
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in &all {
        by_form_id.insert(record.form_id, *record);
    }
    println!("records indexed: {}", by_form_id.len());
    let index = CellIndex::build(&plugin);

    // ---- Part A: interior inn door (the working direction).
    let interior = load_cell(&plugin, INN_EDID, None).expect("load inn interior");
    println!(
        "\n--- Part A: interior '{}' ({} objects, {} doors) ---",
        interior.edid,
        interior.objects.len(),
        interior.doors.len()
    );
    for d in &interior.doors {
        let obj = interior
            .objects
            .iter()
            .find(|o| o.ref_form_id == d.ref_form_id);
        println!(
            "  door ref 0x{:08x} {} trigger Skyrim [{:.0}, {:.0}, {:.0}] Bevy [{:.2}, {:.2}, {:.2}] target {:?} spawn Skyrim [{:.0}, {:.0}, {:.0}] yaw {:.3}",
            d.ref_form_id,
            obj.map(|o| o.base_edid.as_str()).unwrap_or("?"),
            d.trigger_pos[0], d.trigger_pos[1], d.trigger_pos[2],
            d.trigger_pos[0] / UPM, d.trigger_pos[2] / UPM, -d.trigger_pos[1] / UPM,
            d.target,
            d.spawn.position[0], d.spawn.position[1], d.spawn.position[2],
            d.spawn.yaw,
        );
        if let Some(o) = obj {
            println!(
                "    object: base 0x{:08x} {} model {:?} rot [{:.3}, {:.3}, {:.3}] scale {:.2} xtel dest 0x{:08x}",
                o.base_form_id,
                o.base_edid,
                o.model_path,
                o.rotation[0], o.rotation[1], o.rotation[2],
                o.scale,
                o.xtel.map(|x| x.dest_ref).unwrap_or(0),
            );
        }
    }
    // The interior door's XTEL dest IS the exterior door ref.
    // (The load door out is the one whose target is Exterior;
    // room doors have no XTEL and never appear here.)
    let interior_door = interior
        .doors
        .iter()
        .find(|d| matches!(d.target, SceneTarget::Exterior { .. }))
        .or(interior.doors.first())
        .expect("inn has a door");
    let exterior_door_ref = interior
        .objects
        .iter()
        .find(|o| o.ref_form_id == interior_door.ref_form_id)
        .and_then(|o| o.xtel)
        .map(|x| x.dest_ref)
        .expect("interior door has XTEL");
    println!("  interior door's XTEL dest (the EXTERIOR door ref): 0x{exterior_door_ref:08x}");

    // ---- Part B: exterior doors.
    let arrival = find_xtel_spawn(&plugin, INN_EDID);
    if let Some(s) = &arrival {
        println!(
            "\n  exterior arrival spawn: Skyrim [{:.0}, {:.0}, {:.0}] yaw {:.3} -> Bevy [{:.2}, {:.2}, {:.2}]",
            s.position[0], s.position[1], s.position[2], s.yaw,
            s.position[0] / UPM, s.position[2] / UPM, -s.position[1] / UPM
        );
    }
    let exterior = load_exterior(&plugin, (5, -11), arrival).expect("load Riverwood");
    println!(
        "\n--- Part B: exterior '{}' ({} objects, {} doors) ---",
        exterior.edid,
        exterior.objects.len(),
        exterior.doors.len()
    );
    println!(
        "  skipped: markers={} disabled={} no_data={} no_base={} no_model={} other={} effects={}",
        exterior.skipped_markers,
        exterior.skipped_disabled,
        exterior.skipped_no_data,
        exterior.skipped_no_base,
        exterior.skipped_no_model,
        exterior.skipped_other_types,
        exterior.skipped_effects
    );
    for d in &exterior.doors {
        let obj = exterior
            .objects
            .iter()
            .find(|o| o.ref_form_id == d.ref_form_id);
        println!(
            "  door ref 0x{:08x} {} trigger Bevy [{:.2}, {:.2}, {:.2}] target {:?} spawn Skyrim [{:.0}, {:.0}, {:.0}] yaw {:.3}",
            d.ref_form_id,
            obj.map(|o| o.base_edid.as_str()).unwrap_or("?"),
            d.trigger_pos[0] / UPM, d.trigger_pos[2] / UPM, -d.trigger_pos[1] / UPM,
            d.target,
            d.spawn.position[0], d.spawn.position[1], d.spawn.position[2],
            d.spawn.yaw,
        );
    }
    let ext_door_obj = exterior
        .objects
        .iter()
        .find(|o| o.ref_form_id == exterior_door_ref);
    match ext_door_obj {
        Some(o) => {
            println!(
                "\n  EXTERIOR INN DOOR FOUND in objects: ref 0x{:08x} base 0x{:08x} {} model {:?}",
                o.ref_form_id, o.base_form_id, o.base_edid, o.model_path
            );
            println!(
                "    pos Skyrim [{:.1}, {:.1}, {:.1}] Bevy [{:.2}, {:.2}, {:.2}] rot [{:.3}, {:.3}, {:.3}] scale {:.2}",
                o.position[0], o.position[1], o.position[2],
                o.position[0] / UPM, o.position[2] / UPM, -o.position[1] / UPM,
                o.rotation[0], o.rotation[1], o.rotation[2],
                o.scale
            );
            if let Some(b) = o.bounds {
                println!(
                    "    OBND min [{}, {}, {}] max [{}, {}, {}] (X extent {:.2} m)",
                    b.min[0],
                    b.min[1],
                    b.min[2],
                    b.max[0],
                    b.max[1],
                    b.max[2],
                    (b.max[0] - b.min[0]) as f32 / UPM
                );
            } else {
                println!("    OBND: none (door width falls back to 1.4 m)");
            }
            match o.xtel {
                Some(x) => println!(
                    "    XTEL dest_ref 0x{:08x} pos Skyrim [{:.0}, {:.0}, {:.0}] rot [{:.3}, {:.3}, {:.3}]",
                    x.dest_ref, x.pos[0], x.pos[1], x.pos[2], x.rot[0], x.rot[1], x.rot[2]
                ),
                None => println!("    XTEL: NONE — this door has no teleport target!"),
            }
            // Is it in the trigger list?
            let in_triggers = exterior
                .doors
                .iter()
                .any(|d| d.ref_form_id == exterior_door_ref);
            println!("    in cell.doors (build_doors): {in_triggers}");
        }
        None => {
            println!(
                "\n  *** EXTERIOR INN DOOR ref 0x{exterior_door_ref:08x} NOT in exterior objects! ***"
            );
            // Where is the ref? Find its record + parent cell.
            if let Some(r) = by_form_id.get(&exterior_door_ref) {
                println!(
                    "    ref record exists: type {} edid {:?}",
                    r.type_str(),
                    r.edid()
                );
                println!(
                    "    parent cell: {:?}",
                    index.parent_cell_of(exterior_door_ref)
                );
            } else {
                println!("    ref record NOT in Skyrim.esm index at all");
            }
        }
    }

    // ---- Part C: XTEL resolution trace for both inn doors.
    println!("\n--- Part C: XTEL target resolution ---");
    for (label, xtel) in [
        (
            "interior door -> outside",
            interior
                .objects
                .iter()
                .find(|o| o.ref_form_id == interior_door.ref_form_id)
                .and_then(|o| o.xtel),
        ),
        ("exterior door -> inside", ext_door_obj.and_then(|o| o.xtel)),
    ] {
        match xtel {
            Some(x) => {
                println!("  {label}: dest_ref 0x{:08x}", x.dest_ref);
                match index.parent_cell_of(x.dest_ref) {
                    Some(parent) => {
                        println!("    parent_cell_of -> 0x{parent:08x}");
                        match by_form_id.get(&parent) {
                            Some(cell) => {
                                println!(
                                    "    parent record: type {} edid {:?} interior={}",
                                    cell.type_str(),
                                    cell.edid(),
                                    cell_is_interior(cell)
                                );
                                if cell.type_str() == "CELL" && cell_is_interior(cell) {
                                    println!(
                                        "    => resolves to Interior {{ edid: {:?} }}",
                                        cell.edid()
                                    );
                                } else {
                                    println!(
                                        "    => NOT an interior CELL — falls back to Exterior {{ grid: {:?} }}",
                                        grid_of_position(x.pos)
                                    );
                                }
                            }
                            None => println!(
                                "    parent record NOT in index — falls back to Exterior {{ grid: {:?} }}",
                                grid_of_position(x.pos)
                            ),
                        }
                    }
                    None => println!(
                        "    parent_cell_of -> NONE — falls back to Exterior {{ grid: {:?} }} (XTEL pos [{:.0}, {:.0}, {:.0}])",
                        grid_of_position(x.pos),
                        x.pos[0], x.pos[1], x.pos[2]
                    ),
                }
            }
            None => println!("  {label}: no XTEL"),
        }
    }

    // ---- Part D: the door NIFs — hinge nodes + Open sequences.
    // The AssetProvider is the game's own lookup (normalizes
    // case/separators, checks loose files + every BSA).
    println!("\n--- Part D: door NIF hinge/Open check ---");
    let mut provider = AssetProvider::new(Path::new("test-data"));
    println!(
        "  provider indexed {} files from {} archives",
        provider.indexed_files(),
        provider.archive_names.len()
    );
    let mut meshes: HashSet<String> = HashSet::new();
    for bsa in [
        "test-data/Skyrim_-_Meshes0.bsa",
        "test-data/Skyrim_-_Meshes1.bsa",
    ] {
        match BsaArchive::open(bsa) {
            Ok(a) => {
                for stored in a.list() {
                    meshes.insert(stored.replace('\\', "/").to_lowercase());
                }
                println!("  indexed {bsa}: {} files", a.len());
            }
            Err(e) => println!("  warning: could not open {bsa}: {e}"),
        }
    }
    let mesh_exists = |model: &str| {
        let key = format!("meshes/{}", model.replace('\\', "/")).to_lowercase();
        meshes.contains(&key)
    };
    for (label, model) in [
        (
            "interior door",
            interior
                .objects
                .iter()
                .find(|o| o.ref_form_id == interior_door.ref_form_id)
                .and_then(|o| o.model_path.clone()),
        ),
        (
            "exterior door",
            ext_door_obj.and_then(|o| o.model_path.clone()),
        ),
    ] {
        match model {
            Some(m) => {
                println!("  {label}: model {m} exists_in_meshes={}", mesh_exists(&m));
                match provider.read_file(&format!("meshes\\{m}")) {
                    Some(bytes) => match NifFile::from_bytes(&bytes) {
                        Ok(nif) => {
                            let subs = nif.meshes();
                            let hinge_subs: Vec<&str> = subs
                                .iter()
                                .filter(|s| is_hinge_node(&s.parent_node))
                                .map(|s| s.parent_node.as_str())
                                .collect();
                            println!(
                                "    NIF parses: {} sub-meshes, hinge-node sub-meshes: {:?}",
                                subs.len(),
                                hinge_subs
                            );
                            println!(
                                "    parent nodes: {:?}",
                                subs.iter()
                                    .map(|s| s.parent_node.as_str())
                                    .collect::<HashSet<_>>()
                            );
                            match open_angle_from_nif(&nif) {
                                Some((angle, dur)) => println!(
                                    "    Open sequence: angle {:.3} rad ({:.1} deg) over {:.2} s",
                                    angle,
                                    angle.to_degrees(),
                                    dur
                                ),
                                None => println!(
                                    "    Open sequence: NONE — spawn_nif uses the 90 deg fallback"
                                ),
                            }
                            println!(
                                "    => DoorRuntime? {} (needs >=1 hinge-node sub-mesh that converts)",
                                if hinge_subs.is_empty() {
                                    "NO — falls back to the M9 proximity edge"
                                } else {
                                    "YES — swinging door"
                                }
                            );
                        }
                        Err(e) => println!("    NIF parse FAILED: {e}"),
                    },
                    None => println!("    NIF bytes NOT readable from the Meshes BSAs"),
                }
            }
            None => println!("  {label}: no model path"),
        }
    }

    // ---- Part E: placeholder census (exterior).
    println!("\n--- Part E: exterior placeholder census (model missing from Meshes BSAs) ---");
    let mut missing: Vec<(u32, String, String, f32, Vec3, Vec3, Vec3)> = Vec::new();
    let mut with_model = 0usize;
    for o in &exterior.objects {
        let Some(m) = &o.model_path else {
            continue;
        };
        with_model += 1;
        if mesh_exists(m) {
            continue;
        }
        // OBND size in Bevy metres (coords::to_bevy_bounds).
        let (size, _center) = match o.bounds {
            Some(b) => {
                let lo = Vec3::new(
                    b.min[0] as f32 / UPM,
                    b.min[2] as f32 / UPM,
                    -(b.max[1] as f32) / UPM,
                );
                let hi = Vec3::new(
                    b.max[0] as f32 / UPM,
                    b.max[2] as f32 / UPM,
                    -(b.min[1] as f32) / UPM,
                );
                ((hi - lo).max(Vec3::splat(0.05)), (lo + hi) / 2.0)
            }
            None => (Vec3::new(0.3, 0.3, 0.3), Vec3::ZERO),
        };
        let scaled = size * o.scale;
        missing.push((
            o.ref_form_id,
            o.base_edid.clone(),
            o.base_type.clone(),
            o.scale,
            size,
            scaled,
            to_bevy(o.position),
        ));
    }
    missing.sort_by(|a, b| {
        let va = a.5.x * a.5.y * a.5.z;
        let vb = b.5.x * b.5.y * b.5.z;
        vb.partial_cmp(&va).unwrap_or(std::cmp::Ordering::Equal)
    });
    println!(
        "  objects with a model path: {with_model}; model file MISSING: {}",
        missing.len()
    );
    println!("  (shell skip: unscaled volume > 100 m^3 is skipped by the game; scaled volume is what renders)");
    for (ref_id, edid, btype, scale, size, scaled, pos) in &missing {
        let vol = size.x * size.y * size.z;
        let svol = scaled.x * scaled.y * scaled.z;
        println!(
            "  ref 0x{ref_id:08x} {btype:<5} {edid:<40} scale {scale:.2} OBND [{:.2}x{:.2}x{:.2}] {:.1} m^3 -> SCALED [{:.2}x{:.2}x{:.2}] {:.1} m^3 {} Bevy [{:.1}, {:.1}, {:.1}]",
            size.x, size.y, size.z, vol,
            scaled.x, scaled.y, scaled.z, svol,
            if vol > 100.0 { "SKIPPED(shell)" } else { "SPAWNED" },
            pos.x, pos.y, pos.z
        );
    }

    // ---- Part F: walk simulation through the exterior doorway.
    println!("\n--- Part F: walk simulation (exterior door) ---");
    if let (Some(o), Some(arrival)) = (ext_door_obj, &arrival) {
        let q = to_bevy_rot(o.rotation);
        let plane_point = to_bevy(o.position);
        let plane_normal = q * Vec3::NEG_Z;
        let lateral = q * Vec3::X;
        let base_y = plane_point.y;
        let width = o
            .bounds
            .map(|b| (b.max[0] - b.min[0]) as f32 / UPM)
            .filter(|w| *w > 0.0)
            .unwrap_or(1.4);
        println!(
            "  plane_point Bevy [{:.2}, {:.2}, {:.2}] normal [{:.3}, {:.3}, {:.3}] lateral [{:.3}, {:.3}, {:.3}] base_y {:.2} width {:.2}",
            plane_point.x, plane_point.y, plane_point.z,
            plane_normal.x, plane_normal.y, plane_normal.z,
            lateral.x, lateral.y, lateral.z,
            base_y, width
        );
        let spawn_bevy = to_bevy(arrival.position);
        // The REAL door runtime + teleport predicate: the swing
        // values do not affect the crossing (starts_open makes
        // `is_open_enough` pass), so the fallback anim is fine.
        let transform = Transform {
            translation: plane_point,
            rotation: q,
            scale: Vec3::ONE,
        };
        let door = DoorRuntime::new(
            o.ref_form_id,
            &transform,
            width,
            true,
            DoorAnim::fallback(),
            true,
        );
        println!(
            "  arrival spawn Bevy [{:.2}, {:.2}, {:.2}] — horizontal dist to trigger: {:.2} m (proximity radius 1.2 m)",
            spawn_bevy.x, spawn_bevy.y, spawn_bevy.z,
            ((spawn_bevy.x - plane_point.x).powi(2) + (spawn_bevy.z - plane_point.z).powi(2)).sqrt()
        );
        // Walk: spawn -> door threshold -> 3 m past the plane
        // (into the building). The arrival spawn sits on the
        // -normal side of the plane (d = -1.17), so INTO the
        // building is +normal — the first version of this probe
        // walked -normal (further into the street) and trivially
        // never crossed. Sample every 0.1 m. Feet at porch
        // height (base_y) and, separately, at terrain height
        // (base_y - 1.35, the M9 porch offset).
        let target = plane_point + plane_normal * 3.0;
        for (label, feet_y) in [
            ("porch height (base_y)", base_y),
            ("terrain height (base_y - 1.35)", base_y - 1.35),
        ] {
            let start = Vec3::new(spawn_bevy.x, feet_y, spawn_bevy.z);
            let end = Vec3::new(target.x, feet_y, target.z);
            let steps = ((end - start).length() / 0.1).ceil() as usize;
            let mut prev: Option<Vec3> = None;
            let mut crossed_at: Option<usize> = None;
            let mut entered_trigger_at: Option<usize> = None;
            let mut was_inside = false;
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let p = start + (end - start) * t;
                let dx = p.x - plane_point.x;
                let dz = p.z - plane_point.z;
                let horiz = (dx * dx + dz * dz).sqrt();
                let inside = horiz < 1.2 && (p.y - plane_point.y).abs() < 2.0;
                if inside && !was_inside {
                    entered_trigger_at = Some(i);
                }
                was_inside = inside;
                if let Some(pr) = prev {
                    if swinging_door_fires(&door, Some(pr), p) {
                        crossed_at = Some(i);
                        break;
                    }
                }
                prev = Some(p);
            }
            println!(
                "  {label}: crossing fires at step {:?} (of {steps}); proximity edge enters at step {:?}",
                crossed_at, entered_trigger_at
            );
        }
        // Also: does the straight walk pass within USE range?
        let dist_spawn_to_door = ((spawn_bevy.x - plane_point.x).powi(2)
            + (spawn_bevy.z - plane_point.z).powi(2))
        .sqrt();
        println!(
            "  USE range is 2.5 m; spawn is {dist_spawn_to_door:.2} m from the door ref (horizontal)"
        );
    } else {
        println!("  cannot simulate: exterior door object or arrival spawn missing");
    }
    println!("\nprobe done.");
}
