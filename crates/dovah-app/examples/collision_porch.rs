//! Porch-blocker diagnostic: which collision triangles block
//! the first step of the porch climb?
//!
//! Loads the three refs that contribute collision near the
//! porch walk line (WalkwayStairs8 0x0002a74a, the road ramp
//! 0x0001cd8b, the inn 0x0002d763), transforms their collision
//! shapes to world space, and dumps every STEEP triangle /
//! footprint overlapping the cells along the first 2 m of the
//! walk line — with the source ref, so the blocker is named.
//!
//! Usage: cargo run -p dovah-app --example collision_porch
//! (run from the workspace root; reads test-data/)

use dovah_bsa::BsaArchive;
use dovah_esm::{Group, Plugin, Record};
use dovah_nif::{CollisionShape, NifFile};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

const DATA_DIR: &str = "test-data";
const ESM_PATH: &str = "test-data/Skyrim.esm";

// Walk line (Bevy metres): A = deck, B = street.
const WALK_A: (f32, f32) = (332.28, 635.37);
const WALK_B: (f32, f32) = (328.04, 631.56);
// The refs that contribute collision near the line (from
// collision_design Part C2).
const REFS: [u32; 3] = [0x0002a74a, 0x0001cd8b, 0x0002d763];

fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

fn form_id_from_field(record: &Record, field_type: &[u8; 4]) -> Option<u32> {
    let field = record.field(field_type)?;
    if field.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(field.data[0..4].try_into().unwrap()))
}

fn parse_placement(data: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    if data.len() != 24 {
        return None;
    }
    let f = |i: usize| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    Some(([f(0), f(1), f(2)], [f(3), f(4), f(5)]))
}

// Skyrim-space rotation: R = Rz(-r[2]) * Ry(r[1]) * Rx(r[0]).
type Mat3 = [[f32; 3]; 3];
fn rot_x(a: f32) -> Mat3 {
    let (s, c) = a.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}
fn rot_y(a: f32) -> Mat3 {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}
fn rot_z(a: f32) -> Mat3 {
    let (s, c) = a.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}
fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}
fn mat_vec(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Skyrim units -> Bevy metres: (x, y, z) -> (x/70, z/70, -y/70).
fn to_bevy(p: [f32; 3]) -> [f32; 3] {
    [p[0] / 70.0, p[2] / 70.0, -p[1] / 70.0]
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("load Skyrim.esm");
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        for r in g.records() {
            by_form_id.insert(r.form_id, r);
        }
        stack.extend(g.subgroups());
    }
    // Open the Meshes BSAs.
    let mut archives: Vec<BsaArchive<File>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for name in ["Skyrim_-_Meshes0.bsa", "Skyrim_-_Meshes1.bsa"] {
        let path = Path::new(DATA_DIR).join(name);
        let archive = BsaArchive::open(&path).expect("open Meshes BSA");
        let idx = archives.len();
        for stored in archive.list() {
            index.entry(normalize(stored)).or_insert(idx);
        }
        archives.push(archive);
    }
    let mut read_file = |path: &str| -> Option<Vec<u8>> {
        let idx = *index.get(&normalize(path))?;
        if let Ok(bytes) = archives[idx].extract(path) {
            return Some(bytes);
        }
        let stored: String = archives[idx]
            .list()
            .into_iter()
            .find(|s| normalize(s) == normalize(path))?
            .to_string();
        archives[idx].extract(&stored).ok()
    };

    // The walk line's first 2 m, sampled at 0.1 m. A triangle
    // counts as "near the line" when its centroid or a vertex
    // comes within 0.5 m of a sample point (the 1 m cells the
    // line touches).
    let dir = (WALK_A.0 - WALK_B.0, WALK_A.1 - WALK_B.1);
    let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt();
    let dir = (dir.0 / len, dir.1 / len);
    let mut line_points: Vec<(f32, f32)> = Vec::new();
    let mut d = 0.0f32;
    while d <= 2.0 {
        line_points.push((WALK_B.0 + dir.0 * d, WALK_B.1 + dir.1 * d));
        d += 0.1;
    }

    for ref_id in REFS {
        let Some(record) = by_form_id.get(&ref_id) else {
            println!("ref 0x{ref_id:08x}: not found");
            continue;
        };
        let Some((position, rotation)) =
            record.field(b"DATA").and_then(|f| parse_placement(&f.data))
        else {
            continue;
        };
        let Some(base_id) = form_id_from_field(record, b"NAME") else {
            continue;
        };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        let Some(model) = base.field(b"MODL").map(|f| f.as_str()) else {
            continue;
        };
        let nif_path = format!("meshes\\{model}");
        let edid = base.edid().unwrap_or_default();
        println!("\n=== ref 0x{ref_id:08x} {edid} ({nif_path}) ===");
        println!(
            "  pos=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3})",
            position[0], position[1], position[2], rotation[0], rotation[1], rotation[2]
        );
        let Some(bytes) = read_file(&nif_path) else {
            println!("  NIF not found");
            continue;
        };
        let Ok(nif) = NifFile::from_bytes(&bytes) else {
            println!("  NIF parse failed");
            continue;
        };
        let rot = mat_mul(
            &mat_mul(&rot_z(-rotation[2]), &rot_y(rotation[1])),
            &rot_x(rotation[0]),
        );
        let to_world_bevy = |p: [f32; 3]| -> [f32; 3] {
            let r = mat_vec(&rot, p);
            to_bevy([r[0] + position[0], r[1] + position[1], r[2] + position[2]])
        };
        // For each steep triangle / footprint, check whether its
        // 2D footprint comes within 0.5 m of any line point.
        let near_line = |x: f32, z: f32| -> bool {
            line_points
                .iter()
                .any(|(lx, lz)| (x - lx).powi(2) + (z - lz).powi(2) < 0.25)
        };
        let mut steep_near = 0usize;
        let mut steep_total = 0usize;
        for shape in nif.collision_shapes() {
            match shape {
                CollisionShape::Mesh {
                    vertices,
                    triangles,
                    material,
                    layer,
                } => {
                    if ![1u8, 9, 13, 27].contains(&layer) {
                        continue;
                    }
                    for t in &triangles {
                        let (a, b, c) = (
                            to_world_bevy(vertices[t[0] as usize]),
                            to_world_bevy(vertices[t[1] as usize]),
                            to_world_bevy(vertices[t[2] as usize]),
                        );
                        // Bevy-space normal (up = +Y).
                        let ux = b[0] - a[0];
                        let uy = b[1] - a[1];
                        let uz = b[2] - a[2];
                        let vx = c[0] - a[0];
                        let vy = c[1] - a[1];
                        let vz = c[2] - a[2];
                        let nx = uy * vz - uz * vy;
                        let ny = uz * vx - ux * vz;
                        let nz = ux * vy - uy * vx;
                        let l = (nx * nx + ny * ny + nz * nz).sqrt();
                        if l < 1e-9 {
                            continue;
                        }
                        if ny / l >= 0.5 {
                            continue;
                        }
                        steep_total += 1;
                        let cx = (a[0] + b[0] + c[0]) / 3.0;
                        let cz = (a[2] + b[2] + c[2]) / 3.0;
                        if !near_line(cx, cz) {
                            // Also check the triangle's vertices
                            // (a long thin triangle's centroid
                            // can be far from the line while an
                            // end touches it).
                            let verts_near = [a, b, c].iter().any(|v| near_line(v[0], v[2]));
                            if !verts_near {
                                continue;
                            }
                        }
                        steep_near += 1;
                        let min_y = a[1].min(b[1]).min(c[1]);
                        let max_y = a[1].max(b[1]).max(c[1]);
                        println!(
                            "  STEEP mat=0x{material:08x} layer={layer} c=({cx:.2},{cz:.2}) y {min_y:.2}..{max_y:.2} m ny={:.2}",
                            ny / l
                        );
                    }
                }
                CollisionShape::Capsule {
                    a,
                    b,
                    radius,
                    layer,
                } => {
                    if ![1u8, 9, 13, 27].contains(&layer) {
                        continue;
                    }
                    let ab = to_world_bevy(a);
                    let bb = to_world_bevy(b);
                    let r = radius / 70.0;
                    let cx = (ab[0] + bb[0]) / 2.0;
                    let cz = (ab[2] + bb[2]) / 2.0;
                    if near_line(cx, cz) {
                        println!(
                            "  CAPSULE layer={layer} c=({cx:.2},{cz:.2}) y {:.2}..{:.2} m r={r:.2}",
                            ab[1].min(bb[1]) - r,
                            ab[1].max(bb[1]) + r
                        );
                    }
                }
                CollisionShape::Hull { points, layer } => {
                    if ![1u8, 9, 13, 27].contains(&layer) {
                        continue;
                    }
                    let mut min = [f32::INFINITY; 3];
                    let mut max = [f32::NEG_INFINITY; 3];
                    for p in &points {
                        let v = to_world_bevy(*p);
                        for i in 0..3 {
                            min[i] = min[i].min(v[i]);
                            max[i] = max[i].max(v[i]);
                        }
                    }
                    let cx = (min[0] + max[0]) / 2.0;
                    let cz = (min[2] + max[2]) / 2.0;
                    if near_line(cx, cz) || [min, max].iter().any(|v| near_line(v[0], v[2])) {
                        println!(
                            "  HULL layer={layer} c=({cx:.2},{cz:.2}) x {:.2}..{:.2} z {:.2}..{:.2} y {:.2}..{:.2} m",
                            min[0], max[0], min[2], max[2], min[1], max[1]
                        );
                    }
                }
            }
        }
        println!("  steep triangles total: {steep_total}, near the line: {steep_near}");
    }
}
