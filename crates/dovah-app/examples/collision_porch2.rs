//! Porch-blocker diagnostic v2: which REF blocks the first
//! step of the porch climb?
//!
//! The v1 probe checked only the 3 refs with up-facing
//! triangles near the line — the blocker is a STEEP triangle
//! and could come from any ref. This loads ALL exterior refs,
//! and for each one within 15 m of the walk line's start,
//! dumps its steep collision triangles + footprints whose
//! world footprint comes within 1 m of the line's first 1 m —
//! with the ref EDID, so the blocker is named.
//!
//! Usage: cargo run -p dovah-app --example collision_porch2
//! (run from the workspace root; reads test-data/)

use dovah_bsa::BsaArchive;
use dovah_esm::{Group, GroupType, Plugin, Record};
use dovah_nif::{CollisionShape, NifFile};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

const DATA_DIR: &str = "test-data";
const ESM_PATH: &str = "test-data/Skyrim.esm";
const CENTRE: (i32, i32) = (5, -11);
const PERSISTENT_CELL_FORM_ID: u32 = 0x00000d74;
const TAMRIEL_WRLD: u32 = 0x0000003c;
const FLAG_INITIALLY_DISABLED: u32 = 0x00000800;
const FLAG_IS_MARKER: u32 = 0x00000020;

// Walk line (Bevy metres): A = deck, B = street.
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

    // Exterior refs (3x3 + persistent), filtered to 15 m of the
    // walk line's start (Bevy).
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

    // The walk line's first 1 m, sampled at 0.1 m.
    let dir = (WALK_A.0 - WALK_B.0, WALK_A.1 - WALK_B.1);
    let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt();
    let dir = (dir.0 / len, dir.1 / len);
    let mut line_points: Vec<(f32, f32)> = Vec::new();
    let mut d = 0.0f32;
    while d <= 1.0 {
        line_points.push((WALK_B.0 + dir.0 * d, WALK_B.1 + dir.1 * d));
        d += 0.1;
    }
    let near_line = |x: f32, z: f32| -> bool {
        line_points
            .iter()
            .any(|(lx, lz)| (x - lx).powi(2) + (z - lz).powi(2) < 1.0)
    };

    let mut nif_cache: HashMap<String, Option<NifFile>> = HashMap::new();
    let mut checked = 0usize;
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
        checked += 1;
        let edid = base.edid().unwrap_or_default();
        let rot = mat_mul(
            &mat_mul(&rot_z(-rotation[2]), &rot_y(rotation[1])),
            &rot_x(rotation[0]),
        );
        let to_world_bevy = |p: [f32; 3]| -> [f32; 3] {
            let r = mat_vec(&rot, p);
            to_bevy([r[0] + position[0], r[1] + position[1], r[2] + position[2]])
        };
        let mut hits: Vec<String> = Vec::new();
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
                        if l < 1e-9 || ny / l >= 0.5 {
                            continue;
                        }
                        let cx = (a[0] + b[0] + c[0]) / 3.0;
                        let cz = (a[2] + b[2] + c[2]) / 3.0;
                        let verts_near =
                            near_line(cx, cz) || [a, b, c].iter().any(|v| near_line(v[0], v[2]));
                        if !verts_near {
                            continue;
                        }
                        let min_y = a[1].min(b[1]).min(c[1]);
                        let max_y = a[1].max(b[1]).max(c[1]);
                        hits.push(format!(
                            "    STEEP mat=0x{material:08x} c=({cx:.2},{cz:.2}) y {min_y:.2}..{max_y:.2} ny={:.2}",
                            ny / l
                        ));
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
                        hits.push(format!(
                            "    CAPSULE layer={layer} c=({cx:.2},{cz:.2}) y {:.2}..{:.2} r={r:.2}",
                            ab[1].min(bb[1]) - r,
                            ab[1].max(bb[1]) + r
                        ));
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
                    if near_line(cx, cz) {
                        hits.push(format!(
                            "    HULL layer={layer} c=({cx:.2},{cz:.2}) x {:.2}..{:.2} z {:.2}..{:.2} y {:.2}..{:.2}",
                            min[0], max[0], min[2], max[2], min[1], max[1]
                        ));
                    }
                }
            }
        }
        if !hits.is_empty() {
            println!("ref 0x{:08x} {} ({})", r.form_id, edid, nif_path);
            for h in hits.iter().take(12) {
                println!("{h}");
            }
            if hits.len() > 12 {
                println!("    ... and {} more", hits.len() - 12);
            }
        }
    }
    println!("\nrefs within 15 m checked: {checked}");
}
