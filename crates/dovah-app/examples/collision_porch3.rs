//! Porch-blocker diagnostic v3: use the REAL overlap test
//! (`triangle_overlaps_cell_2d` + `point_in_triangle_2d` from
//! collision.rs) against the actual destination cells of the
//! porch climb's first step, and name every overlapping steep
//! triangle / footprint.
//!
//! The destination is 0.1 m from the walk line's street end
//! toward the deck; the obstacle grid is 0.25 m cells over the
//! exterior bounds (origin 234.06, 526.63). The destination
//! sits near a cell corner, so all four cells around it are
//! tested.
//!
//! Usage: cargo run -p dovah-app --example collision_porch3
//! (run from the workspace root; reads test-data/)

use bevy::math::Vec3;
use dovah_app::collision::ObstacleGrid;
use dovah_bsa::BsaArchive;
use dovah_esm::{Group, GroupType, Plugin, Record};
use dovah_nif::{CollisionShape, NifFile};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

const DATA_DIR: &str = "test-data";
const ESM_PATH: &str = "test-data/Skyrim.esm";
const CENTRE: (i32, i32) = (5, -11);
const CELL_SPAN: f32 = 4096.0;
const PERSISTENT_CELL_FORM_ID: u32 = 0x00000d74;
const TAMRIEL_WRLD: u32 = 0x0000003c;
const FLAG_INITIALLY_DISABLED: u32 = 0x00000800;
const FLAG_IS_MARKER: u32 = 0x00000020;

const WALK_A: (f32, f32) = (332.28, 635.37);
const WALK_B: (f32, f32) = (328.04, 631.56);

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
fn is_marker_base(base: &Record) -> bool {
    let edid = base.edid().unwrap_or_default();
    if edid.starts_with("XMarker") || edid.ends_with("Marker") {
        return true;
    }
    if let Some(model) = base.field(b"MODL").map(|f| f.as_str()) {
        let lower = model.to_lowercase();
        let file = lower.rsplit(['\\', '/']).next().unwrap_or(&lower);
        if file.starts_with("marker") || file.contains("leanmarker") {
            return true;
        }
    }
    false
}
struct CellIndex<'a> {
    children: HashMap<u32, Vec<&'a Record>>,
}
impl<'a> CellIndex<'a> {
    fn build(plugin: &'a Plugin) -> Self {
        let mut index = CellIndex {
            children: HashMap::new(),
        };
        for group in &plugin.groups {
            index.scan(group);
        }
        index
    }
    fn scan(&mut self, group: &'a Group) {
        if matches!(
            group.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        ) {
            if let Some(cell_id) = group.label_form_id() {
                let entry = self.children.entry(cell_id).or_default();
                for r in group.records() {
                    entry.push(r);
                }
            }
        }
        for subgroup in group.subgroups() {
            self.scan(subgroup);
        }
    }
    fn children_of(&self, cell_id: u32) -> &[&'a Record] {
        self.children
            .get(&cell_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}
fn tamriel_cells(plugin: &Plugin) -> HashMap<(i32, i32), &Record> {
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    let mut tamriel_group: Option<&Group> = None;
    while let Some(g) = stack.pop() {
        if g.group_type == GroupType::WorldChildren && g.label_form_id() == Some(TAMRIEL_WRLD) {
            tamriel_group = Some(g);
            break;
        }
        stack.extend(g.subgroups());
    }
    let mut by_grid = HashMap::new();
    let Some(tg) = tamriel_group else {
        return by_grid;
    };
    let mut stack2: Vec<&Group> = vec![tg];
    while let Some(g) = stack2.pop() {
        for r in g.records() {
            if r.type_str() == "CELL" && r.form_id != PERSISTENT_CELL_FORM_ID {
                if let Some(f) = r.field(b"XCLC") {
                    if f.data.len() >= 8 {
                        let gx = i32::from_le_bytes(f.data[0..4].try_into().unwrap());
                        let gy = i32::from_le_bytes(f.data[4..8].try_into().unwrap());
                        by_grid.insert((gx, gy), r);
                    }
                }
            }
        }
        stack2.extend(g.subgroups());
    }
    by_grid
}
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
fn to_bevy(p: [f32; 3]) -> [f32; 3] {
    [p[0] / 70.0, p[2] / 70.0, -p[1] / 70.0]
}

fn main() {
    // Destination of the first step + the grid cells around it.
    let dir = (WALK_A.0 - WALK_B.0, WALK_A.1 - WALK_B.1);
    let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt();
    let dir = (dir.0 / len, dir.1 / len);
    let dest = (WALK_B.0 + dir.0 * 0.1, WALK_B.1 + dir.1 * 0.1);
    // Grid origin from RoomBounds::from_exterior((5,-11)):
    // min.x = (5-1)*4096/70, min.z = -(( -11+2)*4096)/70.
    let origin = (
        (5 - 1) as f32 * CELL_SPAN / 70.0,
        -((-11 + 2) as f32 * CELL_SPAN / 70.0),
    );
    let cell = ObstacleGrid::CELL_SIZE;
    let col = ((dest.0 - origin.0) / cell).floor() as i32;
    let row = ((dest.1 - origin.1) / cell).floor() as i32;
    println!(
        "destination ({:.3}, {:.3}), grid origin ({:.2}, {:.2}), cell size {}, cell (col {col}, row {row})",
        dest.0, dest.1, origin.0, origin.1, cell
    );
    // The four cells around the destination (it sits near a
    // corner, and f32 rounding could put it in any of them).
    let mut test_cells: Vec<(f32, f32, f32, f32)> = Vec::new();
    for dc in [0, -1] {
        for dr in [0, -1] {
            let x0 = origin.0 + (col + dc) as f32 * cell;
            let z0 = origin.1 + (row + dr) as f32 * cell;
            test_cells.push((x0, x0 + cell, z0, z0 + cell));
            println!(
                "  test cell x {x0:.2}..{:.2} z {z0:.2}..{:.2}",
                x0 + cell,
                z0 + cell
            );
        }
    }
    // The band for the first step: ground is the base plate
    // (-1.89) or the landing (-1.80); band = [ground+0.3,
    // ground+1.7]. Report any interval overlapping [-1.59, -0.10]
    // (the union over both grounds).
    println!("  band (union over grounds -1.89/-1.80): [-1.59, -0.10]");

    let plugin = Plugin::open(ESM_PATH).expect("load Skyrim.esm");
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        for r in g.records() {
            by_form_id.insert(r.form_id, r);
        }
        stack.extend(g.subgroups());
    }
    let cells = CellIndex::build(&plugin);
    let grids = tamriel_cells(&plugin);
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
    let start_sky = [WALK_B.0 * 70.0, -WALK_B.1 * 70.0];
    let mut refs: Vec<&Record> = Vec::new();
    for gx in CENTRE.0 - 1..=CENTRE.0 + 1 {
        for gy in CENTRE.1 - 1..=CENTRE.1 + 1 {
            if let Some(cell) = grids.get(&(gx, gy)) {
                refs.extend(cells.children_of(cell.form_id));
            }
        }
    }
    refs.extend(cells.children_of(PERSISTENT_CELL_FORM_ID));
    let mut nif_cache: HashMap<String, Option<NifFile>> = HashMap::new();
    // Re-implement the grid's overlap test locally (the helpers
    // are pub(crate), not reachable from examples): centre
    // covered OR SAT overlap. Copy the logic exactly.
    fn point_in_triangle_2d(px: f32, pz: f32, a: Vec3, b: Vec3, c: Vec3) -> bool {
        let d1 = (px - b.x) * (a.z - b.z) - (pz - b.z) * (a.x - b.x);
        let d2 = (px - c.x) * (b.z - c.z) - (pz - c.z) * (b.x - c.x);
        let d3 = (px - a.x) * (c.z - a.z) - (pz - a.z) * (c.x - a.x);
        let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(has_neg && has_pos)
    }
    fn triangle_overlaps_cell_2d(
        a: Vec3,
        b: Vec3,
        c: Vec3,
        x0: f32,
        x1: f32,
        z0: f32,
        z1: f32,
    ) -> bool {
        for (p, q) in [(a, b), (b, c), (c, a)] {
            let ex = q.x - p.x;
            let ez = q.z - p.z;
            let nx = -ez;
            let nz = ex;
            if nx == 0.0 && nz == 0.0 {
                continue;
            }
            let t0 = a.x * nx + a.z * nz;
            let t1 = b.x * nx + b.z * nz;
            let t2 = c.x * nx + c.z * nz;
            let tmin = t0.min(t1).min(t2);
            let tmax = t0.max(t1).max(t2);
            let c0 = x0 * nx + z0 * nz;
            let c1 = x1 * nx + z0 * nz;
            let c2 = x0 * nx + z1 * nz;
            let c3 = x1 * nx + z1 * nz;
            let bmin = c0.min(c1).min(c2).min(c3);
            let bmax = c0.max(c1).max(c2).max(c3);
            if tmax < bmin || bmax < tmin {
                return false;
            }
        }
        true
    }
    let overlaps_any_cell = |a: Vec3, b: Vec3, c: Vec3| -> bool {
        for (x0, x1, z0, z1) in &test_cells {
            let cx = (x0 + x1) / 2.0;
            let cz = (z0 + z1) / 2.0;
            if point_in_triangle_2d(cx, cz, a, b, c)
                || triangle_overlaps_cell_2d(a, b, c, *x0, *x1, *z0, *z1)
            {
                return true;
            }
        }
        false
    };
    let mut hits = 0usize;
    for r in refs {
        if r.type_str() != "REFR" {
            continue;
        }
        if r.flags & FLAG_INITIALLY_DISABLED != 0 || r.flags & FLAG_IS_MARKER != 0 {
            continue;
        }
        let Some((position, rotation)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data))
        else {
            continue;
        };
        let dx = position[0] - start_sky[0];
        let dy = position[1] - start_sky[1];
        if (dx * dx + dy * dy).sqrt() > 15.0 * 70.0 {
            continue;
        }
        let Some(base_id) = form_id_from_field(r, b"NAME") else {
            continue;
        };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        if is_marker_base(base) {
            continue;
        }
        let Some(model) = base.field(b"MODL").map(|f| f.as_str()) else {
            continue;
        };
        let nif_path = format!("meshes\\{model}");
        let nif = match nif_cache.get(&nif_path) {
            Some(n) => n.as_ref(),
            None => {
                let n = read_file(&nif_path).and_then(|b| NifFile::from_bytes(&b).ok());
                nif_cache.insert(nif_path.clone(), n);
                nif_cache.get(&nif_path).unwrap().as_ref()
            }
        };
        let Some(nif) = nif else { continue };
        let edid = base.edid().unwrap_or_default();
        let rot = mat_mul(
            &mat_mul(&rot_z(-rotation[2]), &rot_y(rotation[1])),
            &rot_x(rotation[0]),
        );
        let to_world_bevy = |p: [f32; 3]| -> Vec3 {
            let r = mat_vec(&rot, p);
            let b = to_bevy([r[0] + position[0], r[1] + position[1], r[2] + position[2]]);
            Vec3::from(b)
        };
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
                        let normal = (b - a).cross(c - a);
                        if normal.length_squared() < 1e-12 {
                            continue;
                        }
                        let n = normal.normalize();
                        if n.y >= 0.5 {
                            continue;
                        }
                        if !overlaps_any_cell(a, b, c) {
                            continue;
                        }
                        let min_y = a.y.min(b.y).min(c.y);
                        let max_y = a.y.max(b.y).max(c.y);
                        let in_band = max_y > -1.59 && min_y < -0.10;
                        println!(
                            "  ref 0x{:08x} {} mat=0x{material:08x} c=({:.2},{:.2}) y {min_y:.2}..{max_y:.2} ny={:.2} IN_BAND={in_band}",
                            r.form_id,
                            edid,
                            (a.x + b.x + c.x) / 3.0,
                            (a.z + b.z + c.z) / 3.0,
                            n.y
                        );
                        hits += 1;
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
                    let rr = radius / 70.0;
                    let (min_x, max_x) = (ab.x.min(bb.x) - rr, ab.x.max(bb.x) + rr);
                    let (min_z, max_z) = (ab.z.min(bb.z) - rr, ab.z.max(bb.z) + rr);
                    let (min_y, max_y) = (ab.y.min(bb.y) - rr, ab.y.max(bb.y) + rr);
                    for (x0, x1, z0, z1) in &test_cells {
                        if min_x < *x1 && max_x > *x0 && min_z < *z1 && max_z > *z0 {
                            let in_band = max_y > -1.59 && min_y < -0.10;
                            println!(
                                "  ref 0x{:08x} {} CAPSULE y {min_y:.2}..{max_y:.2} IN_BAND={in_band}",
                                r.form_id, edid
                            );
                            hits += 1;
                            break;
                        }
                    }
                }
                CollisionShape::Hull { points, layer } => {
                    if ![1u8, 9, 13, 27].contains(&layer) {
                        continue;
                    }
                    let mut min = Vec3::splat(f32::INFINITY);
                    let mut max = Vec3::splat(f32::NEG_INFINITY);
                    for p in &points {
                        let v = to_world_bevy(*p);
                        min = min.min(v);
                        max = max.max(v);
                    }
                    for (x0, x1, z0, z1) in &test_cells {
                        if min.x < *x1 && max.x > *x0 && min.z < *z1 && max.z > *z0 {
                            let in_band = max.y > -1.59 && min.y < -0.10;
                            println!(
                                "  ref 0x{:08x} {} HULL y {:.2}..{:.2} IN_BAND={in_band}",
                                r.form_id, edid, min.y, max.y
                            );
                            hits += 1;
                            break;
                        }
                    }
                }
            }
        }
    }
    println!("\noverlapping steep shapes in the destination cells: {hits}");
}
