//! Collision design probe: answer the questions the step-up
//! design depends on.
//!
//! A. LAYER CENSUS: any STAIRHELPER (31) bodies in Riverwood?
//!    (per-shape layer counts over every unique NIF)
//! B. MATERIAL CENSUS: which Havok material codes appear, and
//!    in which NIFs (is the stairs-only 0x571ff595 the stair
//!    material?).
//! C. PORCH: Inn01's collision triangles in WORLD space near the
//!    porch walk line (from step_height_probe) — which material
//!    are the porch steps, and what are their heights?
//! D. FURNITURE: bench/chair/bed collision — materials + seat
//!    heights (the thing the step-up must NOT climb).
//!
//! Usage: cargo run -p dovah-app --example collision_design
//! (run from the workspace root; reads test-data/)

use dovah_bsa::BsaArchive;
use dovah_esm::{Group, GroupType, Plugin, Record};
use dovah_nif::{CollisionShape, NifFile};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::path::Path;

const DATA_DIR: &str = "test-data";
const ESM_PATH: &str = "test-data/Skyrim.esm";
const CENTRE: (i32, i32) = (5, -11);
const CELL_SPAN: f32 = 4096.0;
const PERSISTENT_CELL_FORM_ID: u32 = 0x00000d74;
const TAMRIEL_WRLD: u32 = 0x0000003c;
const INN_CELL_FORM_ID: u32 = 0x000133c6;
const INN_REF_FORM_ID: u32 = 0x0002d763;

const FLAG_INITIALLY_DISABLED: u32 = 0x00000800;
const FLAG_IS_MARKER: u32 = 0x00000020;

// Porch walk line (Skyrim units), from step_height_probe's
// offset-0.0 profile: spawn (deck) -> street.
const WALK_A: [f32; 2] = [23259.6, -44475.9];
const WALK_B: [f32; 2] = [22962.8, -44209.2];

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

fn ref_scale(record: &Record) -> f32 {
    record
        .field(b"XSCL")
        .and_then(|f| {
            if f.data.len() == 4 {
                Some(f32::from_le_bytes(f.data[0..4].try_into().unwrap()))
            } else {
                None
            }
        })
        .unwrap_or(1.0)
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

struct Archives {
    archives: Vec<BsaArchive<File>>,
    index: HashMap<String, usize>,
}

impl Archives {
    fn open_all(data_dir: &Path) -> Self {
        let mut paths: Vec<_> = fs::read_dir(data_dir)
            .expect("read data dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .map(|e| e.eq_ignore_ascii_case("bsa"))
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();
        let mut out = Archives {
            archives: Vec::new(),
            index: HashMap::new(),
        };
        for path in paths {
            match BsaArchive::open(&path) {
                Ok(archive) => {
                    let idx = out.archives.len();
                    for stored in archive.list() {
                        out.index.entry(normalize(stored)).or_insert(idx);
                    }
                    out.archives.push(archive);
                }
                Err(e) => eprintln!("warning: could not open {}: {e}", path.display()),
            }
        }
        out
    }

    fn read_file(&mut self, path: &str) -> Option<Vec<u8>> {
        let idx = *self.index.get(&normalize(path))?;
        if let Ok(bytes) = self.archives[idx].extract(path) {
            return Some(bytes);
        }
        let stored: String = self.archives[idx]
            .list()
            .into_iter()
            .find(|s| normalize(s) == normalize(path))?
            .to_string();
        self.archives[idx].extract(&stored).ok()
    }
}

struct PlacedRef {
    form_id: u32,
    base_edid: String,
    model_path: String,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: f32,
}

fn collect_refs(records: &[&Record], by_form_id: &HashMap<u32, &Record>, out: &mut Vec<PlacedRef>) {
    for r in records {
        let kind = r.type_str();
        if kind != "REFR" && kind != "ACHR" {
            continue;
        }
        if r.flags & FLAG_INITIALLY_DISABLED != 0 || r.flags & FLAG_IS_MARKER != 0 {
            continue;
        }
        let Some((position, rotation)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data))
        else {
            continue;
        };
        let Some(base_id) = form_id_from_field(r, b"NAME") else {
            continue;
        };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        if is_marker_base(base) {
            continue;
        }
        if kind == "ACHR" {
            continue;
        }
        let Some(model) = base.field(b"MODL").map(|f| f.as_str()) else {
            continue;
        };
        out.push(PlacedRef {
            form_id: r.form_id,
            base_edid: base.edid().unwrap_or_default(),
            model_path: format!("meshes\\{model}"),
            position,
            rotation,
            scale: ref_scale(r),
        });
    }
}

// --- Skyrim-space rotation: R = Rz(-r[2]) * Ry(r[1]) * Rx(r[0])
// (coords.rs convention). Row-major 3x3.
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
fn ref_rotation(r: [f32; 3]) -> Mat3 {
    let z = rot_z(-r[2]);
    let y = rot_y(r[1]);
    let x = rot_x(r[0]);
    mat_mul(&mat_mul(&z, &y), &x)
}

fn dist_point_segment_2d(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [p[0] - a[0], p[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 < 1e-9 {
        0.0
    } else {
        ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0)
    };
    let closest = [a[0] + t * ab[0], a[1] + t * ab[1]];
    ((p[0] - closest[0]).powi(2) + (p[1] - closest[1]).powi(2)).sqrt()
}

fn main() {
    let plugin = Plugin::open(Path::new(ESM_PATH)).expect("load Skyrim.esm");
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
    let mut archives = Archives::open_all(Path::new(DATA_DIR));

    let mut ext_refs: Vec<PlacedRef> = Vec::new();
    for gx in CENTRE.0 - 1..=CENTRE.0 + 1 {
        for gy in CENTRE.1 - 1..=CENTRE.1 + 1 {
            if let Some(cell) = grids.get(&(gx, gy)) {
                collect_refs(cells.children_of(cell.form_id), &by_form_id, &mut ext_refs);
            }
        }
    }
    collect_refs(
        cells.children_of(PERSISTENT_CELL_FORM_ID),
        &by_form_id,
        &mut ext_refs,
    );
    let (x0, x1) = (
        (CENTRE.0 - 1) as f32 * CELL_SPAN,
        (CENTRE.0 + 2) as f32 * CELL_SPAN,
    );
    let (y0, y1) = (
        (CENTRE.1 - 1) as f32 * CELL_SPAN,
        (CENTRE.1 + 2) as f32 * CELL_SPAN,
    );
    ext_refs.retain(|r| {
        r.position[0] >= x0 && r.position[0] < x1 && r.position[1] >= y0 && r.position[1] < y1
    });
    let mut inn_refs: Vec<PlacedRef> = Vec::new();
    collect_refs(
        cells.children_of(INN_CELL_FORM_ID),
        &by_form_id,
        &mut inn_refs,
    );
    println!(
        "loaded: {} exterior refs, {} inn refs",
        ext_refs.len(),
        inn_refs.len()
    );

    let mut nif_cache: HashMap<String, Option<NifFile>> = HashMap::new();
    let mut unique_paths: HashSet<String> = HashSet::new();
    for r in ext_refs.iter().chain(inn_refs.iter()) {
        unique_paths.insert(r.model_path.clone());
    }
    for path in &unique_paths {
        let nif = archives
            .read_file(path)
            .and_then(|b| NifFile::from_bytes(&b).ok());
        nif_cache.insert(path.clone(), nif);
    }

    // ---- A. layer census ----
    println!("\n=== A. COLLISION SHAPE LAYERS (unique NIFs) ===");
    let mut layer_shapes: BTreeMap<u8, usize> = BTreeMap::new();
    let mut layer_nifs: BTreeMap<u8, HashSet<String>> = BTreeMap::new();
    let mut shapes_by_nif: HashMap<String, Vec<CollisionShape>> = HashMap::new();
    for (path, nif) in &nif_cache {
        let Some(nif) = nif else { continue };
        let shapes = nif.collision_shapes();
        for s in &shapes {
            *layer_shapes.entry(s.layer()).or_insert(0) += 1;
            layer_nifs
                .entry(s.layer())
                .or_default()
                .insert(path.clone());
        }
        shapes_by_nif.insert(path.clone(), shapes);
    }
    for (layer, count) in &layer_shapes {
        println!(
            "  layer {layer:2}: {count:4} shapes in {} NIFs",
            layer_nifs[layer].len()
        );
    }
    if let Some(nifs) = layer_nifs.get(&31) {
        println!("  STAIRHELPER (31) NIFs:");
        for n in nifs {
            println!("    {n}");
        }
    } else {
        println!("  NO layer-31 (STAIRHELPER) shapes anywhere in Riverwood.");
    }

    // ---- B. material census ----
    println!("\n=== B. HAVOK MATERIALS (compressed meshes, unique NIFs) ===");
    let mut mat_nifs: BTreeMap<u32, HashSet<String>> = BTreeMap::new();
    let mut mat_tris: BTreeMap<u32, usize> = BTreeMap::new();
    for (path, shapes) in &shapes_by_nif {
        for s in shapes {
            if let CollisionShape::Mesh {
                triangles,
                material,
                ..
            } = s
            {
                mat_nifs.entry(*material).or_default().insert(path.clone());
                *mat_tris.entry(*material).or_insert(0) += triangles.len();
            }
        }
    }
    for (mat, nifs) in &mat_nifs {
        println!(
            "  mat 0x{mat:08x}: {:6} tris in {} NIFs",
            mat_tris[mat],
            nifs.len()
        );
    }
    // Which NIFs carry the stairs-only material?
    if let Some(nifs) = mat_nifs.get(&0x571ff595) {
        println!("  0x571ff595 appears in:");
        for n in nifs {
            println!("    {n}");
        }
    }

    // ---- C. porch: Inn01 collision in world space ----
    println!("\n=== C. PORCH (Inn01 collision, world space) ===");
    let Some(inn_ref) = ext_refs.iter().find(|r| r.form_id == INN_REF_FORM_ID) else {
        println!("  inn ref 0x0002d763 not found in exterior refs");
        return;
    };
    println!(
        "  inn ref: pos=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3}) scale={:.2} NIF={}",
        inn_ref.position[0],
        inn_ref.position[1],
        inn_ref.position[2],
        inn_ref.rotation[0],
        inn_ref.rotation[1],
        inn_ref.rotation[2],
        inn_ref.scale,
        inn_ref.model_path,
    );
    let rot = ref_rotation(inn_ref.rotation);
    let to_world = |p: [f32; 3]| -> [f32; 3] {
        let scaled = [
            p[0] * inn_ref.scale,
            p[1] * inn_ref.scale,
            p[2] * inn_ref.scale,
        ];
        let r = mat_vec(&rot, scaled);
        [
            r[0] + inn_ref.position[0],
            r[1] + inn_ref.position[1],
            r[2] + inn_ref.position[2],
        ]
    };
    let Some(shapes) = shapes_by_nif.get(&inn_ref.model_path) else {
        println!("  inn NIF not in cache");
        return;
    };
    // Up-facing world-space triangles within 1.5 m of the walk
    // line, grouped by height (cm) + material.
    let mut near: Vec<(f32, u32, [f32; 3])> = Vec::new();
    for s in shapes {
        let CollisionShape::Mesh {
            vertices,
            triangles,
            material,
            ..
        } = s
        else {
            continue;
        };
        for t in triangles {
            let (a, b, c) = (
                to_world(vertices[t[0] as usize]),
                to_world(vertices[t[1] as usize]),
                to_world(vertices[t[2] as usize]),
            );
            // World-space normal (z = up in Skyrim space).
            let ux = b[0] - a[0];
            let uy = b[1] - a[1];
            let uz = b[2] - a[2];
            let vx = c[0] - a[0];
            let vy = c[1] - a[1];
            let vz = c[2] - a[2];
            let nz = ux * vy - uy * vx;
            let len = ((uy * vz - uz * vy).powi(2) + (uz * vx - ux * vz).powi(2) + nz * nz).sqrt();
            if len < 1e-9 || nz / len < 0.5 {
                continue;
            }
            let centroid = [
                (a[0] + b[0] + c[0]) / 3.0,
                (a[1] + b[1] + c[1]) / 3.0,
                (a[2] + b[2] + c[2]) / 3.0,
            ];
            let d = dist_point_segment_2d([centroid[0], centroid[1]], WALK_A, WALK_B);
            if d > 105.0 {
                continue;
            }
            near.push((centroid[2], *material, centroid));
        }
    }
    near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    println!(
        "  up-facing Inn01 collision tris within 1.5 m of walk line: {}",
        near.len()
    );
    // Group by height band (2 cm) + material.
    let mut bands: BTreeMap<(i32, u32), (usize, f32, f32)> = BTreeMap::new();
    for (z, mat, c) in &near {
        let band = (z / 1.4).round() as i32; // ~2 cm bands
        let entry = bands
            .entry((band, *mat))
            .or_insert((0, f32::INFINITY, f32::NEG_INFINITY));
        entry.0 += 1;
        entry.1 = entry.1.min(c[0]);
        entry.2 = entry.2.max(c[0]);
    }
    for ((band, mat), (n, xmin, xmax)) in &bands {
        println!(
            "  z={:7.1}u ({:6.2} m) mat=0x{mat:08x} n={n:3} x {xmin:.0}..{xmax:.0}",
            *band as f32 * 1.4,
            *band as f32 * 1.4 / 70.0,
        );
    }
    if near.is_empty() {
        println!("  (none — the porch steps may belong to a different ref; see Part C2)");
    }

    // C2: ALL refs near the walk line — which NIFs contribute
    // up-facing collision triangles there?
    println!("\n  --- C2: all refs contributing near the walk line ---");
    let mut contributors: BTreeMap<String, (u32, usize, f32, f32)> = BTreeMap::new();
    for pref in &ext_refs {
        let Some(shapes) = shapes_by_nif.get(&pref.model_path) else {
            continue;
        };
        if shapes.is_empty() {
            continue;
        };
        let rot = ref_rotation(pref.rotation);
        let to_world = |p: [f32; 3]| -> [f32; 3] {
            let scaled = [p[0] * pref.scale, p[1] * pref.scale, p[2] * pref.scale];
            let r = mat_vec(&rot, scaled);
            [
                r[0] + pref.position[0],
                r[1] + pref.position[1],
                r[2] + pref.position[2],
            ]
        };
        for s in shapes {
            let CollisionShape::Mesh {
                vertices,
                triangles,
                material,
                ..
            } = s
            else {
                continue;
            };
            for t in triangles {
                let (a, b, c) = (
                    to_world(vertices[t[0] as usize]),
                    to_world(vertices[t[1] as usize]),
                    to_world(vertices[t[2] as usize]),
                );
                let ux = b[0] - a[0];
                let uy = b[1] - a[1];
                let uz = b[2] - a[2];
                let vx = c[0] - a[0];
                let vy = c[1] - a[1];
                let vz = c[2] - a[2];
                let nz = ux * vy - uy * vx;
                let len =
                    ((uy * vz - uz * vy).powi(2) + (uz * vx - ux * vz).powi(2) + nz * nz).sqrt();
                if len < 1e-9 || nz / len < 0.5 {
                    continue;
                }
                let centroid = [
                    (a[0] + b[0] + c[0]) / 3.0,
                    (a[1] + b[1] + c[1]) / 3.0,
                    (a[2] + b[2] + c[2]) / 3.0,
                ];
                let d = dist_point_segment_2d([centroid[0], centroid[1]], WALK_A, WALK_B);
                if d > 105.0 {
                    continue;
                }
                let key = format!("0x{:08x} {}", pref.form_id, pref.base_edid);
                let entry = contributors.entry(key).or_insert((
                    *material,
                    0,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                ));
                entry.1 += 1;
                entry.2 = entry.2.min(centroid[2]);
                entry.3 = entry.3.max(centroid[2]);
            }
        }
    }
    for (key, (mat, n, zmin, zmax)) in &contributors {
        println!(
            "  {key}: mat=0x{mat:08x} n={n} z {zmin:.0}..{zmax:.0}u ({:.2}..{:.2} m)",
            zmin / 70.0,
            zmax / 70.0
        );
    }
    if contributors.is_empty() {
        println!("  (no ref contributes up-facing collision near the walk line)");
    }

    // ---- E. WalkwayStairs8 profile: the porch steps, world space ----
    println!("\n=== E. PORCH STEPS (WalkwayStairs8 0x0002a74a, world space) ===");
    if let Some(stairs_ref) = ext_refs.iter().find(|r| r.form_id == 0x0002a74a) {
        println!(
            "  ref: pos=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3}) scale={:.2}",
            stairs_ref.position[0],
            stairs_ref.position[1],
            stairs_ref.position[2],
            stairs_ref.rotation[0],
            stairs_ref.rotation[1],
            stairs_ref.rotation[2],
            stairs_ref.scale,
        );
        let rot = ref_rotation(stairs_ref.rotation);
        let to_world = |p: [f32; 3]| -> [f32; 3] {
            let scaled = [
                p[0] * stairs_ref.scale,
                p[1] * stairs_ref.scale,
                p[2] * stairs_ref.scale,
            ];
            let r = mat_vec(&rot, scaled);
            [
                r[0] + stairs_ref.position[0],
                r[1] + stairs_ref.position[1],
                r[2] + stairs_ref.position[2],
            ]
        };
        if let Some(shapes) = shapes_by_nif.get(&stairs_ref.model_path) {
            // Up-facing tris within 1.5 m of the walk line, sorted
            // by distance along the line (t from A=spawn to
            // B=street), with height + material.
            let mut steps: Vec<(f32, f32, u32)> = Vec::new();
            for s in shapes {
                let CollisionShape::Mesh {
                    vertices,
                    triangles,
                    material,
                    ..
                } = s
                else {
                    continue;
                };
                for t in triangles {
                    let (a, b, c) = (
                        to_world(vertices[t[0] as usize]),
                        to_world(vertices[t[1] as usize]),
                        to_world(vertices[t[2] as usize]),
                    );
                    let ux = b[0] - a[0];
                    let uy = b[1] - a[1];
                    let uz = b[2] - a[2];
                    let vx = c[0] - a[0];
                    let vy = c[1] - a[1];
                    let vz = c[2] - a[2];
                    let nz = ux * vy - uy * vx;
                    let len = ((uy * vz - uz * vy).powi(2) + (uz * vx - ux * vz).powi(2) + nz * nz)
                        .sqrt();
                    if len < 1e-9 || nz / len < 0.5 {
                        continue;
                    }
                    let centroid = [
                        (a[0] + b[0] + c[0]) / 3.0,
                        (a[1] + b[1] + c[1]) / 3.0,
                        (a[2] + b[2] + c[2]) / 3.0,
                    ];
                    let d = dist_point_segment_2d([centroid[0], centroid[1]], WALK_A, WALK_B);
                    if d > 105.0 {
                        continue;
                    }
                    // t along the line: 0 at A (spawn/deck), 1 at
                    // B (street). Invert so t grows walking UP.
                    let ab = [WALK_B[0] - WALK_A[0], WALK_B[1] - WALK_A[1]];
                    let ap = [centroid[0] - WALK_A[0], centroid[1] - WALK_A[1]];
                    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
                    let t = 1.0 - (ap[0] * ab[0] + ap[1] * ab[1]) / len2;
                    steps.push((t, centroid[2], *material));
                }
            }
            steps.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            println!("  up-facing tris near line: {}", steps.len());
            let mut prev_z: Option<f32> = None;
            for (t, z, mat) in &steps {
                let rise = prev_z.map(|p| z - p).unwrap_or(0.0);
                println!(
                    "  t={t:.2} z={z:7.1}u ({:6.2} m) mat=0x{mat:08x} rise={rise:+.1}u ({:+.2} m)",
                    z / 70.0,
                    rise / 70.0,
                );
                prev_z = Some(*z);
            }
        }
    } else {
        println!("  WalkwayStairs8 ref 0x0002a74a not found");
    }

    // ---- F. all stair-named refs (exterior + inn) ----
    println!("\n=== F. STAIR-NAMED REFS ===");
    for (where_, refs) in [("EXT", &ext_refs), ("INN", &inn_refs)] {
        let mut seen: HashSet<String> = HashSet::new();
        for pref in refs {
            let hay = format!("{} {}", pref.base_edid, pref.model_path).to_lowercase();
            if !hay.contains("stair") {
                continue;
            }
            if !seen.insert(pref.model_path.clone()) {
                continue;
            }
            println!(
                "  {where_} 0x{:08x} {} {}",
                pref.form_id, pref.base_edid, pref.model_path
            );
        }
    }

    // ---- D. furniture ----
    println!("\n=== D. FURNITURE COLLISION ===");
    for needle in [
        "farmtablebench01",
        "noblechair01",
        "woodenchair01",
        "commonbed01",
        "farmbench01",
    ] {
        let mut found = false;
        for (path, shapes) in &shapes_by_nif {
            if !path.to_lowercase().contains(needle) {
                continue;
            }
            found = true;
            println!("  {path}: {} shapes", shapes.len());
            for s in shapes {
                match s {
                    CollisionShape::Mesh {
                        vertices,
                        triangles,
                        material,
                        layer,
                    } => {
                        // Highest up-facing triangle (seat top).
                        let mut top = f32::NEG_INFINITY;
                        let mut up_tris = 0usize;
                        for t in triangles {
                            let (a, b, c) = (
                                vertices[t[0] as usize],
                                vertices[t[1] as usize],
                                vertices[t[2] as usize],
                            );
                            let ux = b[0] - a[0];
                            let uy = b[1] - a[1];
                            let uz = b[2] - a[2];
                            let vx = c[0] - a[0];
                            let vy = c[1] - a[1];
                            let vz = c[2] - a[2];
                            let nz = ux * vy - uy * vx;
                            let len = ((uy * vz - uz * vy).powi(2)
                                + (uz * vx - ux * vz).powi(2)
                                + nz * nz)
                                .sqrt();
                            if len < 1e-9 || nz / len < 0.5 {
                                continue;
                            }
                            up_tris += 1;
                            let cz = (a[2] + b[2] + c[2]) / 3.0;
                            top = top.max(cz);
                        }
                        println!(
                            "    Mesh mat=0x{material:08x} layer={layer} verts={} tris={} up={} top_up={:.1}u ({:.2} m)",
                            vertices.len(),
                            triangles.len(),
                            up_tris,
                            top,
                            top / 70.0
                        );
                    }
                    CollisionShape::Capsule {
                        a,
                        b,
                        radius,
                        layer,
                    } => {
                        println!(
                            "    Capsule layer={layer} a=({:.0},{:.0},{:.0}) b=({:.0},{:.0},{:.0}) r={:.0}u",
                            a[0], a[1], a[2], b[0], b[1], b[2], radius
                        );
                    }
                    CollisionShape::Hull { points, layer } => {
                        let mut zmin = f32::INFINITY;
                        let mut zmax = f32::NEG_INFINITY;
                        for p in points {
                            zmin = zmin.min(p[2]);
                            zmax = zmax.max(p[2]);
                        }
                        println!(
                            "    Hull layer={layer} pts={} z {zmin:.0}..{zmax:.0}u",
                            points.len()
                        );
                    }
                }
            }
        }
        if !found {
            println!("  {needle}: not in Riverwood NIF set");
        }
    }
}
