//! v0.4.7 phone-feedback probe: identify by REF what renders
//! yellow / black / bright / streaky, and whether the exterior
//! white is mist or overexposure.
//!
//! A. STRETCHED GEOMETRY: every exterior + inn shape whose
//!    world-space AABB has any dimension > 25 m (the yellow/red
//!    poles and white streaks are stretched triangles — find
//!    their refs).
//! B. BRIDGE AREA: every ref within 40 m of the bridge, then a
//!    material dump (textures resolved? vertex-color mean,
//!    alpha flags, unlit?) for fence-ish refs.
//! C. INN TABLES + DOOR: FURN refs with table/counter in the
//!    EDID + door 0x00013419 — NIF shape dump with normal
//!    direction stats (is the table top's normal inverted?) and
//!    vertex-color stats (is the table multiplied black?).
//! D. MIST PLANES: fog/mist refs in the exterior — position,
//!    size, vertex-alpha stats, textures, unlit flag.
//! E. ORGNAR'S HEAD: FaceGen NIF 00013479 — are the head
//!    normals pointing outward or inward (inward = black face)?
//!
//! Usage: cargo run -p dovah-app --example v047_probe

use dovah_bsa::BsaArchive;
use dovah_esm::{Group, GroupType, Plugin, Record};
use dovah_nif::NifFile;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::Path;

const DATA_DIR: &str = "test-data";
const ESM_PATH: &str = "test-data/Skyrim.esm";
const CENTRE: (i32, i32) = (5, -11);
const CELL_SPAN: f32 = 4096.0;
const PERSISTENT_CELL_FORM_ID: u32 = 0x00000d74;
const TAMRIEL_WRLD: u32 = 0x0000003c;
const INN_CELL_FORM_ID: u32 = 0x000133c6;
const INN_DOOR_FORM_ID: u32 = 0x00013419;

const FLAG_INITIALLY_DISABLED: u32 = 0x00000800;
const FLAG_IS_MARKER: u32 = 0x00000020;

type M3 = [[f32; 3]; 3];

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

/// OBND bounds (min/max Skyrim units) from a base record, if present.
fn obnd_bounds(base: &Record) -> Option<([f32; 3], [f32; 3])> {
    let f = base.field(b"OBND")?;
    if f.data.len() < 12 {
        return None;
    }
    let g = |i: usize| i16::from_le_bytes(f.data[i * 2..i * 2 + 2].try_into().unwrap()) as f32;
    Some(([g(0), g(1), g(2)], [g(3), g(4), g(5)]))
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

    fn has_file(&self, path: &str) -> bool {
        self.index.contains_key(&normalize(path))
    }
}

fn mat_vec(m: &M3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn mat_mul(a: &M3, b: &M3) -> M3 {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}

fn rot_x(a: f32) -> M3 {
    let (s, c) = (a.sin(), a.cos());
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

fn rot_y(a: f32) -> M3 {
    let (s, c) = (a.sin(), a.cos());
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

fn rot_z(a: f32) -> M3 {
    let (s, c) = (a.sin(), a.cos());
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

fn ref_rotation(rot: [f32; 3]) -> M3 {
    mat_mul(&mat_mul(&rot_z(rot[2]), &rot_y(rot[1])), &rot_x(rot[0]))
}

struct PlacedRef {
    form_id: u32,
    base_type: String,
    base_edid: String,
    model_path: String,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: f32,
    obnd: Option<([f32; 3], [f32; 3])>,
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
        let scale = r
            .field(b"XSCL")
            .and_then(|f| {
                if f.data.len() == 4 {
                    Some(f32::from_le_bytes(f.data[0..4].try_into().unwrap()))
                } else {
                    None
                }
            })
            .unwrap_or(1.0);
        out.push(PlacedRef {
            form_id: r.form_id,
            base_type: base.type_str(),
            base_edid: base.edid().unwrap_or_default(),
            model_path: format!("meshes\\{model}"),
            position,
            rotation,
            scale,
            obnd: obnd_bounds(base),
        });
    }
}

/// World-space AABB of a shape's vertices under a REFR transform.
/// Mirrors convert.rs (rot * v * scale + trans, then ref transform).
fn shape_world_aabb(
    mesh: &dovah_nif::Mesh,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: f32,
) -> ([f32; 3], [f32; 3]) {
    let rot = mesh.transform.rotation;
    let mscale = mesh.transform.scale;
    let trans = mesh.transform.translation;
    let rrot = ref_rotation(rotation);
    let mut wmin = [f32::INFINITY; 3];
    let mut wmax = [f32::NEG_INFINITY; 3];
    for v in &mesh.vertices {
        let r = mat_vec(&rot, *v);
        let local = [
            r[0] * mscale + trans[0],
            r[1] * mscale + trans[1],
            r[2] * mscale + trans[2],
        ];
        let w = mat_vec(
            &rrot,
            [local[0] * scale, local[1] * scale, local[2] * scale],
        );
        for i in 0..3 {
            let p = w[i] + position[i];
            wmin[i] = wmin[i].min(p);
            wmax[i] = wmax[i].max(p);
        }
    }
    (wmin, wmax)
}

fn vcolor_stats(colors: &[[f32; 4]]) -> (usize, [f32; 4], [f32; 4]) {
    if colors.is_empty() {
        return (0, [0.0; 4], [0.0; 4]);
    }
    let n = colors.len() as f32;
    let mut mean = [0.0f32; 4];
    let mut min = [f32::INFINITY; 4];
    for c in colors {
        for i in 0..4 {
            mean[i] += c[i] / n;
            min[i] = min[i].min(c[i]);
        }
    }
    (colors.len(), mean, min)
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

    // Exterior 3x3 refs + persistent cell.
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
    // The persistent cell holds refs from ALL of Tamriel — keep
    // only the ones inside the 3x3 block (grids 4..6, -12..-10).
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

    // ---- Part A: stretched geometry ----
    println!("\n=== A. STRETCHED SHAPES (any world dim > 25 m) ===");
    let mut nif_cache: HashMap<String, Option<NifFile>> = HashMap::new();
    let mut stretched = 0usize;
    for (where_, refs) in [("EXT", &ext_refs), ("INN", &inn_refs)] {
        for pref in refs {
            let entry = nif_cache.entry(pref.model_path.clone()).or_insert_with(|| {
                archives
                    .read_file(&pref.model_path)
                    .and_then(|b| NifFile::from_bytes(&b).ok())
            });
            let Some(nif) = entry else { continue };
            for mesh in nif.meshes() {
                if mesh.vertices.is_empty() || mesh.indices.is_empty() {
                    continue;
                }
                let (wmin, wmax) =
                    shape_world_aabb(&mesh, pref.position, pref.rotation, pref.scale);
                let size = [
                    (wmax[0] - wmin[0]) / 70.0,
                    (wmax[1] - wmin[1]) / 70.0,
                    (wmax[2] - wmin[2]) / 70.0,
                ];
                if size[0].max(size[1]).max(size[2]) > 25.0 {
                    stretched += 1;
                    let obnd_size = pref.obnd.map(|(mn, mx)| {
                        [
                            (mx[0] - mn[0]) / 70.0,
                            (mx[1] - mn[1]) / 70.0,
                            (mx[2] - mn[2]) / 70.0,
                        ]
                    });
                    println!(
                        "  [{}] ref 0x{:08x} {} {} scale={:.2} shape '{}' skinned={} world size {:.1}x{:.1}x{:.1} m world z [{:.1},{:.1}] m OBND {:?}",
                        where_,
                        pref.form_id,
                        pref.base_type,
                        pref.base_edid,
                        pref.scale,
                        mesh.name,
                        mesh.skinned,
                        size[0],
                        size[1],
                        size[2],
                        wmin[2] / 70.0,
                        wmax[2] / 70.0,
                        obnd_size,
                    );
                }
            }
        }
    }
    println!("  stretched shapes: {stretched}");

    // ---- Part B: bridge area ----
    println!("\n=== B. BRIDGE AREA ===");
    let bridge = ext_refs
        .iter()
        .find(|r| r.base_edid.to_lowercase().contains("bridge"));
    let Some(bridge) = bridge else {
        println!("  no bridge ref found");
        return;
    };
    println!(
        "  bridge ref 0x{:08x} {} at ({:.0},{:.0},{:.0})",
        bridge.form_id,
        bridge.base_edid,
        bridge.position[0],
        bridge.position[1],
        bridge.position[2]
    );
    let bp = bridge.position;
    let mut near: Vec<&PlacedRef> = ext_refs
        .iter()
        .filter(|r| {
            let dx = r.position[0] - bp[0];
            let dy = r.position[1] - bp[1];
            (dx * dx + dy * dy).sqrt() < 40.0 * 70.0
        })
        .collect();
    near.sort_by_key(|r| r.form_id);
    println!("  {} refs within 40 m of the bridge:", near.len());
    for r in &near {
        println!(
            "    0x{:08x} {:<5} {:<40} scale={:.2} pos=({:.0},{:.0},{:.0})",
            r.form_id,
            r.base_type,
            r.base_edid,
            r.scale,
            r.position[0],
            r.position[1],
            r.position[2]
        );
    }
    // Material dump for fence-ish + yellow-suspect refs near the bridge.
    println!("\n  --- material dump: fence / flat / yellow-suspect refs ---");
    let mut dumped: HashSet<String> = HashSet::new();
    for r in &near {
        let edid = r.base_edid.to_lowercase();
        let model = r.model_path.to_lowercase();
        let suspect = edid.contains("fence")
            || edid.contains("bridge")
            || edid.contains("road")
            || edid.contains("splash")
            || edid.contains("rapid")
            || edid.contains("salmon")
            || edid.contains("fish")
            || edid.contains("cliff")
            || edid.contains("rockpile")
            || edid.contains("marker")
            || model.contains("fence")
            || model.contains("bridge")
            || model.contains("road");
        if !suspect || !dumped.insert(r.model_path.clone()) {
            continue;
        }
        dump_nif_materials(&mut archives, &r.model_path, &r.base_edid);
    }

    // ---- Part C: inn tables + counter + door ----
    println!("\n=== C. INN TABLES / COUNTER / DOOR ===");
    for r in &inn_refs {
        let edid = r.base_edid.to_lowercase();
        let is_table = edid.contains("table") || edid.contains("counter");
        let is_door = r.form_id == INN_DOOR_FORM_ID;
        if !is_table && !is_door {
            continue;
        }
        println!(
            "\n  ref 0x{:08x} {} {} scale={:.2}",
            r.form_id, r.base_type, r.base_edid, r.scale
        );
        dump_nif_materials(&mut archives, &r.model_path, &r.base_edid);
        // Normal-direction stats per shape (table top: normals up or down?).
        if let Some(bytes) = archives.read_file(&r.model_path) {
            if let Ok(nif) = NifFile::from_bytes(&bytes) {
                for mesh in nif.meshes() {
                    if mesh.vertices.is_empty() || mesh.normals.is_empty() {
                        continue;
                    }
                    let rot = mesh.transform.rotation;
                    let mut up = 0usize;
                    let mut down = 0usize;
                    let mut mean = [0.0f32; 3];
                    for n in &mesh.normals {
                        let rn = mat_vec(&rot, *n);
                        mean[0] += rn[0];
                        mean[1] += rn[1];
                        mean[2] += rn[2];
                        if rn[2] > 0.7 {
                            up += 1;
                        } else if rn[2] < -0.7 {
                            down += 1;
                        }
                    }
                    let n = mesh.normals.len() as f32;
                    println!(
                        "    shape '{}': {} verts, mean normal ({:.2},{:.2},{:.2}), up {:.0}% down {:.0}%",
                        mesh.name,
                        mesh.normals.len(),
                        mean[0] / n,
                        mean[1] / n,
                        mean[2] / n,
                        100.0 * up as f32 / n,
                        100.0 * down as f32 / n,
                    );
                }
            }
        }
    }

    // ---- Part D: mist planes ----
    println!("\n=== D. MIST / FOG REFS (exterior) ===");
    let mut mist_nifs: HashSet<String> = HashSet::new();
    for r in &ext_refs {
        let edid = r.base_edid.to_lowercase();
        let model = r.model_path.to_lowercase();
        if !(edid.contains("fog") || edid.contains("mist") || model.contains("fog")) {
            continue;
        }
        println!(
            "  ref 0x{:08x} {} {} scale={:.2} pos=({:.0},{:.0},{:.0}) model={}",
            r.form_id,
            r.base_type,
            r.base_edid,
            r.scale,
            r.position[0],
            r.position[1],
            r.position[2],
            r.model_path
        );
        if mist_nifs.insert(r.model_path.clone()) {
            dump_nif_materials(&mut archives, &r.model_path, &r.base_edid);
        }
    }

    // ---- Part E: Orgnar's head normals ----
    println!("\n=== E. ORGNAR HEAD NORMALS (FaceGen 00013479) ===");
    let facegen = "meshes\\actors\\character\\facegendata\\facegeom\\skyrim.esm\\00013479.nif";
    match archives.read_file(facegen) {
        Some(bytes) => match NifFile::from_bytes(&bytes) {
            Ok(nif) => {
                for mesh in nif.meshes() {
                    if mesh.vertices.is_empty() || mesh.normals.is_empty() {
                        continue;
                    }
                    // Outward test: dot(normal, vertex - centre) > 0 means
                    // the normal points away from the mesh centre.
                    let rot = mesh.transform.rotation;
                    let mscale = mesh.transform.scale;
                    let trans = mesh.transform.translation;
                    let mut centre = [0.0f32; 3];
                    let mut pts: Vec<[f32; 3]> = Vec::with_capacity(mesh.vertices.len());
                    for v in &mesh.vertices {
                        let r = mat_vec(&rot, *v);
                        let p = [
                            r[0] * mscale + trans[0],
                            r[1] * mscale + trans[1],
                            r[2] * mscale + trans[2],
                        ];
                        centre[0] += p[0];
                        centre[1] += p[1];
                        centre[2] += p[2];
                        pts.push(p);
                    }
                    let n = pts.len() as f32;
                    centre = [centre[0] / n, centre[1] / n, centre[2] / n];
                    let mut outward = 0usize;
                    let mut mean_dot = 0.0f32;
                    for (i, p) in pts.iter().enumerate() {
                        let rn = mat_vec(&rot, mesh.normals[i]);
                        let d = [p[0] - centre[0], p[1] - centre[1], p[2] - centre[2]];
                        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                        if len < 1e-6 {
                            continue;
                        }
                        let dot = rn[0] * d[0] / len + rn[1] * d[1] / len + rn[2] * d[2] / len;
                        mean_dot += dot;
                        if dot > 0.0 {
                            outward += 1;
                        }
                    }
                    let (nc, vmean, vmin) = vcolor_stats(&mesh.vertex_colors);
                    println!(
                        "  shape '{}': {} verts skinned={} outward {:.0}% mean dot {:.2} | vcolors n={} mean=({:.2},{:.2},{:.2},{:.2}) min=({:.2},{:.2},{:.2},{:.2})",
                        mesh.name,
                        mesh.vertices.len(),
                        mesh.skinned,
                        100.0 * outward as f32 / pts.len() as f32,
                        mean_dot / pts.len() as f32,
                        nc,
                        vmean[0],
                        vmean[1],
                        vmean[2],
                        vmean[3],
                        vmin[0],
                        vmin[1],
                        vmin[2],
                        vmin[3],
                    );
                }
            }
            Err(e) => println!("  FaceGen parse failed: {e}"),
        },
        None => println!("  FaceGen file not found"),
    }

    // Also: which BSEffectShaderProperty blocks exist in the mist NIFs
    // (the palette-lookup question) — dump block type names for newFog.
    println!("\n=== F. newFog NIF block types ===");
    let fog_nif = "meshes\\effects\\newfog.nif";
    match archives.read_file(fog_nif) {
        Some(bytes) => match NifFile::from_bytes(&bytes) {
            Ok(nif) => {
                let mut types: HashMap<String, usize> = HashMap::new();
                for b in &nif.blocks {
                    *types.entry(b.type_name().to_string()).or_insert(0) += 1;
                }
                let mut types: Vec<_> = types.into_iter().collect();
                types.sort();
                for (t, n) in types {
                    println!("  {t}: {n}");
                }
                for mesh in nif.meshes() {
                    let (nc, vmean, vmin) = vcolor_stats(&mesh.vertex_colors);
                    println!(
                        "  shape '{}': {} verts, unlit={} blend={:?} cutoff={:?} diffuse={:?} gradient={:?} vcolors n={} mean=({:.2},{:.2},{:.2},{:.2}) min a={:.2}",
                        mesh.name,
                        mesh.vertices.len(),
                        mesh.unlit,
                        mesh.alpha_blend,
                        mesh.alpha_cutoff,
                        mesh.textures.diffuse,
                        mesh.textures.gradient,
                        nc,
                        vmean[0],
                        vmean[1],
                        vmean[2],
                        vmean[3],
                        vmin[3],
                    );
                }
            }
            Err(e) => println!("  newFog parse failed: {e}"),
        },
        None => println!("  newFog.nif not found at {fog_nif}"),
    }

    // ---- Part G1: newFog path search (the BSA index is keyed by
    // normalized path, so search the keys) ----
    println!("\n=== G1. newFog path search ===");
    let mut fog_paths: Vec<&String> = archives
        .index
        .keys()
        .filter(|k| k.contains("newfog"))
        .collect();
    fog_paths.sort();
    for p in &fog_paths {
        println!("  {p}");
    }
    if fog_paths.is_empty() {
        println!("  (no 'newfog' path in any BSA)");
    }
    let mut fog_effects: Vec<&String> = archives
        .index
        .keys()
        .filter(|k| k.contains("effects/") && k.contains("fog"))
        .collect();
    fog_effects.sort();
    for p in &fog_effects {
        println!("  {p}");
    }

    // ---- Part G2: EditorMarker / emitShape census ----
    println!("\n=== G2. EditorMarker / emitShape census ===");
    let mut marker_count = 0usize;
    for (where_, refs) in [("EXT", &ext_refs), ("INN", &inn_refs)] {
        for pref in refs {
            let entry = nif_cache.entry(pref.model_path.clone()).or_insert_with(|| {
                archives
                    .read_file(&pref.model_path)
                    .and_then(|b| NifFile::from_bytes(&b).ok())
            });
            let Some(nif) = entry else { continue };
            for mesh in nif.meshes() {
                let lname = mesh.name.to_lowercase();
                if !(lname == "editormarker" || lname.starts_with("emitshape")) {
                    continue;
                }
                if mesh.vertices.is_empty() {
                    continue;
                }
                let (wmin, wmax) =
                    shape_world_aabb(&mesh, pref.position, pref.rotation, pref.scale);
                let size = [
                    (wmax[0] - wmin[0]) / 70.0,
                    (wmax[1] - wmin[1]) / 70.0,
                    (wmax[2] - wmin[2]) / 70.0,
                ];
                let centre = [
                    (wmin[0] + wmax[0]) / 2.0 / 70.0,
                    (wmin[1] + wmax[1]) / 2.0 / 70.0,
                    (wmin[2] + wmax[2]) / 2.0 / 70.0,
                ];
                let (nc, vmean, _vmin) = vcolor_stats(&mesh.vertex_colors);
                marker_count += 1;
                println!(
                    "  [{}] ref 0x{:08x} {} {} shape '{}' {} verts size {:.2}x{:.2}x{:.2} m centre ({:.1},{:.1},{:.1}) m unlit={} blend={:?} vcol n={} mean=({:.2},{:.2},{:.2},{:.2})",
                    where_,
                    pref.form_id,
                    pref.base_type,
                    pref.base_edid,
                    mesh.name,
                    mesh.vertices.len(),
                    size[0],
                    size[1],
                    size[2],
                    centre[0],
                    centre[1],
                    centre[2],
                    mesh.unlit,
                    mesh.alpha_blend,
                    nc,
                    vmean[0],
                    vmean[1],
                    vmean[2],
                    vmean[3],
                );
            }
        }
    }
    println!("  marker shapes total: {marker_count}");

    // ---- Part G3: rogue triangles (one long edge inside a normal
    // shape) — the poles/streaks if the marker census doesn't
    // explain them ----
    println!("\n=== G3. ROGUE TRIANGLES (edge > 8 m inside a <25 m shape) ===");
    let mut rogue = 0usize;
    for (where_, refs) in [("EXT", &ext_refs), ("INN", &inn_refs)] {
        for pref in refs {
            let entry = nif_cache.entry(pref.model_path.clone()).or_insert_with(|| {
                archives
                    .read_file(&pref.model_path)
                    .and_then(|b| NifFile::from_bytes(&b).ok())
            });
            let Some(nif) = entry else { continue };
            for mesh in nif.meshes() {
                if mesh.vertices.is_empty() || mesh.indices.is_empty() {
                    continue;
                }
                let (wmin, wmax) =
                    shape_world_aabb(&mesh, pref.position, pref.rotation, pref.scale);
                let maxdim = (wmax[0] - wmin[0])
                    .max(wmax[1] - wmin[1])
                    .max(wmax[2] - wmin[2])
                    / 70.0;
                if maxdim > 25.0 {
                    continue; // legit big shapes, covered by Part A
                }
                let rot = mesh.transform.rotation;
                let mscale = mesh.transform.scale;
                let trans = mesh.transform.translation;
                let rrot = ref_rotation(pref.rotation);
                let world: Vec<[f32; 3]> = mesh
                    .vertices
                    .iter()
                    .map(|v| {
                        let r = mat_vec(&rot, *v);
                        let local = [
                            r[0] * mscale + trans[0],
                            r[1] * mscale + trans[1],
                            r[2] * mscale + trans[2],
                        ];
                        let w = mat_vec(
                            &rrot,
                            [
                                local[0] * pref.scale,
                                local[1] * pref.scale,
                                local[2] * pref.scale,
                            ],
                        );
                        [
                            w[0] + pref.position[0],
                            w[1] + pref.position[1],
                            w[2] + pref.position[2],
                        ]
                    })
                    .collect();
                let edge = |p: [f32; 3], q: [f32; 3]| {
                    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
                        / 70.0
                };
                for tri in mesh.indices.chunks(3) {
                    if tri.len() < 3 {
                        continue;
                    }
                    let (a, b, c) = (
                        world[tri[0] as usize],
                        world[tri[1] as usize],
                        world[tri[2] as usize],
                    );
                    let e = edge(a, b).max(edge(b, c)).max(edge(c, a));
                    if e > 8.0 {
                        rogue += 1;
                        if rogue <= 40 {
                            let ctr = [
                                (a[0] + b[0] + c[0]) / 3.0 / 70.0,
                                (a[1] + b[1] + c[1]) / 3.0 / 70.0,
                                (a[2] + b[2] + c[2]) / 3.0 / 70.0,
                            ];
                            println!(
                                "  [{}] ref 0x{:08x} {} {} shape '{}' max edge {:.1} m tri centre ({:.1},{:.1},{:.1}) m",
                                where_,
                                pref.form_id,
                                pref.base_type,
                                pref.base_edid,
                                mesh.name,
                                e,
                                ctr[0],
                                ctr[1],
                                ctr[2],
                            );
                        }
                    }
                }
            }
        }
    }
    println!("  rogue triangles total: {rogue}");

    // ---- Part G4: FaceGen full shape lists (does the skin mesh
    // exist? Part E only listed shapes WITH normals) ----
    println!("\n=== G4. FaceGen full shape lists ===");
    for (who, form) in [("Orgnar", "00013479"), ("Delphine", "00013478")] {
        let path =
            format!("meshes\\actors\\character\\facegendata\\facegeom\\skyrim.esm\\{form}.nif");
        match archives.read_file(&path) {
            Some(bytes) => match NifFile::from_bytes(&bytes) {
                Ok(nif) => {
                    println!(
                        "  {who} ({form}): {} meshes, {} blocks",
                        nif.meshes().len(),
                        nif.blocks.len()
                    );
                    for mesh in nif.meshes() {
                        let mut lmin = [f32::INFINITY; 3];
                        let mut lmax = [f32::NEG_INFINITY; 3];
                        for v in &mesh.vertices {
                            for i in 0..3 {
                                lmin[i] = lmin[i].min(v[i]);
                                lmax[i] = lmax[i].max(v[i]);
                            }
                        }
                        println!(
                            "    shape '{}': {} verts {} tris skinned={} normals={} local size {:.1}x{:.1}x{:.1} u diffuse={:?}",
                            mesh.name,
                            mesh.vertices.len(),
                            mesh.indices.len() / 3,
                            mesh.skinned,
                            mesh.normals.len(),
                            lmax[0] - lmin[0],
                            lmax[1] - lmin[1],
                            lmax[2] - lmin[2],
                            mesh.textures.diffuse,
                        );
                    }
                }
                Err(e) => println!("  {who}: parse failed: {e}"),
            },
            None => println!("  {who}: file not found"),
        }
    }

    // ---- Part G5: rapids refs + material dumps ----
    println!("\n=== G5. Rapids refs + materials ===");
    let mut rapids_nifs: HashSet<String> = HashSet::new();
    for r in &ext_refs {
        if r.base_edid.to_lowercase().contains("rapids") {
            println!(
                "  ref 0x{:08x} {} {} scale={:.2} pos=({:.0},{:.0},{:.0}) model={}",
                r.form_id,
                r.base_type,
                r.base_edid,
                r.scale,
                r.position[0],
                r.position[1],
                r.position[2],
                r.model_path
            );
            rapids_nifs.insert(r.model_path.clone());
        }
    }
    for model in &rapids_nifs {
        dump_nif_materials(&mut archives, model, "rapids");
    }

    // ---- Part H: head winding test. The FaceGen skin mesh has
    // NO normals (G4) — convert.rs computes smooth normals from
    // the triangle winding. If the skinning path flips the
    // winding, the computed normals point INWARD and the face
    // renders black. Test: compute smooth normals for the head
    // (outward dot vs centre) and for the hair (computed vs
    // STORED normals — hair has both, so it calibrates the
    // test). Also census every normal-less shape in the block.
    println!("\n=== H. Head winding test + normal-less census ===");
    let facegen = "meshes\\actors\\character\\facegendata\\facegeom\\skyrim.esm\\00013479.nif";
    if let Some(bytes) = archives.read_file(facegen) {
        if let Ok(nif) = NifFile::from_bytes(&bytes) {
            for mesh in nif.meshes() {
                if mesh.name != "MaleHeadNord" && mesh.name != "HairMaleNord06" {
                    continue;
                }
                // Smooth normals from winding (same math as
                // Bevy's compute_smooth_normals: face normal =
                // cross(b-a, c-a), accumulated per vertex).
                let mut acc = vec![[0.0f32; 3]; mesh.vertices.len()];
                for tri in mesh.indices.chunks(3) {
                    if tri.len() < 3 {
                        continue;
                    }
                    let (a, b, c) = (
                        mesh.vertices[tri[0] as usize],
                        mesh.vertices[tri[1] as usize],
                        mesh.vertices[tri[2] as usize],
                    );
                    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                    let fn_ = [
                        u[1] * v[2] - u[2] * v[1],
                        u[2] * v[0] - u[0] * v[2],
                        u[0] * v[1] - u[1] * v[0],
                    ];
                    for idx in tri {
                        let s = &mut acc[*idx as usize];
                        s[0] += fn_[0];
                        s[1] += fn_[1];
                        s[2] += fn_[2];
                    }
                }
                let mut centre = [0.0f32; 3];
                for p in &mesh.vertices {
                    centre[0] += p[0];
                    centre[1] += p[1];
                    centre[2] += p[2];
                }
                let n = mesh.vertices.len() as f32;
                centre = [centre[0] / n, centre[1] / n, centre[2] / n];
                let mut outward = 0usize;
                let mut mean_dot = 0.0f32;
                let mut stored_dot = 0.0f32;
                let mut stored_n = 0usize;
                for (i, p) in mesh.vertices.iter().enumerate() {
                    let a = acc[i];
                    let len = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
                    if len < 1e-9 {
                        continue;
                    }
                    let cn = [a[0] / len, a[1] / len, a[2] / len];
                    let d = [p[0] - centre[0], p[1] - centre[1], p[2] - centre[2]];
                    let dl = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    if dl < 1e-6 {
                        continue;
                    }
                    let dot = cn[0] * d[0] / dl + cn[1] * d[1] / dl + cn[2] * d[2] / dl;
                    mean_dot += dot;
                    if dot > 0.0 {
                        outward += 1;
                    }
                    if i < mesh.normals.len() {
                        let s = mesh.normals[i];
                        let sl = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
                        if sl > 1e-6 {
                            stored_dot += cn[0] * s[0] / sl + cn[1] * s[1] / sl + cn[2] * s[2] / sl;
                            stored_n += 1;
                        }
                    }
                }
                println!(
                    "  shape '{}': computed-normal outward {:.0}% mean dot {:.2} | computed-vs-stored mean dot {:.2} (n={})",
                    mesh.name,
                    100.0 * outward as f32 / mesh.vertices.len() as f32,
                    mean_dot / mesh.vertices.len() as f32,
                    if stored_n > 0 {
                        stored_dot / stored_n as f32
                    } else {
                        f32::NAN
                    },
                    stored_n,
                );
            }
        }
    }
    // Census: every shape with vertices but NO normals.
    let mut no_normal: HashMap<String, usize> = HashMap::new();
    let mut no_normal_total = 0usize;
    for refs in [&ext_refs, &inn_refs] {
        for pref in refs.iter() {
            let entry = nif_cache.entry(pref.model_path.clone()).or_insert_with(|| {
                archives
                    .read_file(&pref.model_path)
                    .and_then(|b| NifFile::from_bytes(&b).ok())
            });
            let Some(nif) = entry else { continue };
            for mesh in nif.meshes() {
                if !mesh.vertices.is_empty() && mesh.normals.is_empty() {
                    no_normal_total += 1;
                    *no_normal
                        .entry(format!("{} :: {}", pref.base_edid, mesh.name))
                        .or_insert(0) += 1;
                }
            }
        }
    }
    println!("  normal-less shapes (scene refs): {no_normal_total}");
    let mut nn: Vec<_> = no_normal.into_iter().collect();
    nn.sort_by_key(|a| std::cmp::Reverse(a.1));
    for (k, v) in nn.iter().take(20) {
        println!("    {v}x {k}");
    }
    // NPC bodies/heads are not scene refs — check the body NIF
    // and a generic head directly.
    for (label, path) in [
        (
            "MaleBody",
            "meshes\\actors\\character\\characterassets\\malebody_1.nif",
        ),
        (
            "FemaleBody",
            "meshes\\actors\\character\\characterassets\\femalebody_1.nif",
        ),
        (
            "MaleHead",
            "meshes\\actors\\character\\characterassets\\malehead.nif",
        ),
    ] {
        match archives.read_file(path) {
            Some(bytes) => match NifFile::from_bytes(&bytes) {
                Ok(nif) => {
                    for mesh in nif.meshes() {
                        println!(
                            "  {label} shape '{}': {} verts normals={} skinned={}",
                            mesh.name,
                            mesh.vertices.len(),
                            mesh.normals.len(),
                            mesh.skinned,
                        );
                    }
                }
                Err(e) => println!("  {label}: parse failed: {e}"),
            },
            None => println!("  {label}: not found at {path}"),
        }
    }

    // ---- Part I: inn NPC + light positions (Bevy metres, for
    // --camera placements) + FaceGen shape centres (is the head
    // misplaced vs the hair/beard?) ----
    println!("\n=== I. INN NPCs + LIGHTS + DOOR (Bevy m) ===");
    // Bevy m from Skyrim units: (x/70, z/70, -y/70).
    let bevy = |p: [f32; 3]| [p[0] / 70.0, p[2] / 70.0, -p[1] / 70.0];
    for r in cells.children_of(INN_CELL_FORM_ID) {
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
        let b = bevy(position);
        if kind == "ACHR" {
            println!(
                "  ACHR ref 0x{:08x} NPC 0x{:08x} {} pos Sky=({:.0},{:.0},{:.0}) Bevy=({:.2},{:.2},{:.2}) yaw={:.3}",
                r.form_id,
                base_id,
                base.edid().unwrap_or_default(),
                position[0],
                position[1],
                position[2],
                b[0],
                b[1],
                b[2],
                rotation[2],
            );
            continue;
        }
        if base.type_str() == "LIGH" {
            let (radius, color, flags, fade) = match base.field(b"DATA") {
                Some(f) if f.data.len() >= 16 => (
                    u32::from_le_bytes(f.data[4..8].try_into().unwrap()),
                    u32::from_le_bytes(f.data[8..12].try_into().unwrap()),
                    u32::from_le_bytes(f.data[12..16].try_into().unwrap()),
                    base.field(b"FNAM").and_then(|g| {
                        if g.data.len() == 4 {
                            Some(f32::from_le_bytes(g.data[0..4].try_into().unwrap()))
                        } else {
                            None
                        }
                    }),
                ),
                _ => (0, 0, 0, None),
            };
            println!(
                "  LIGH ref 0x{:08x} {} pos Bevy=({:.2},{:.2},{:.2}) radius={}u color=0x{:08x} flags=0x{:04x} shadow_omni={} fade={:?}",
                r.form_id,
                base.edid().unwrap_or_default(),
                b[0],
                b[1],
                b[2],
                radius,
                color,
                flags,
                flags & 0x1000 != 0,
                fade,
            );
            continue;
        }
        if r.form_id == INN_DOOR_FORM_ID
            || base
                .edid()
                .unwrap_or_default()
                .to_lowercase()
                .contains("counter")
        {
            println!(
                "  ref 0x{:08x} {} {} pos Sky=({:.0},{:.0},{:.0}) Bevy=({:.2},{:.2},{:.2}) yaw={:.3}",
                r.form_id,
                base.type_str(),
                base.edid().unwrap_or_default(),
                position[0],
                position[1],
                position[2],
                b[0],
                b[1],
                b[2],
                rotation[2],
            );
        }
    }
    println!("\n=== I2. FaceGen shape centres (local units) ===");
    if let Some(bytes) = archives.read_file(facegen) {
        if let Ok(nif) = NifFile::from_bytes(&bytes) {
            for mesh in nif.meshes() {
                if mesh.vertices.is_empty() {
                    continue;
                }
                let mut mn = [f32::INFINITY; 3];
                let mut mx = [f32::NEG_INFINITY; 3];
                // Transformed centre: mesh.transform is applied by
                // the renderer, so the raw-vertex centre misleads
                // for shapes parented under the Head node.
                let mut tmn = [f32::INFINITY; 3];
                let mut tmx = [f32::NEG_INFINITY; 3];
                let rot = mesh.transform.rotation;
                let mscale = mesh.transform.scale;
                let trans = mesh.transform.translation;
                for v in &mesh.vertices {
                    for i in 0..3 {
                        mn[i] = mn[i].min(v[i]);
                        mx[i] = mx[i].max(v[i]);
                    }
                    let r = mat_vec(&rot, *v);
                    let p = [
                        r[0] * mscale + trans[0],
                        r[1] * mscale + trans[1],
                        r[2] * mscale + trans[2],
                    ];
                    for i in 0..3 {
                        tmn[i] = tmn[i].min(p[i]);
                        tmx[i] = tmx[i].max(p[i]);
                    }
                }
                println!(
                    "  shape '{}': raw centre ({:.1},{:.1},{:.1}) | transform trans=({:.1},{:.1},{:.1}) scale={:.2} | TRANSFORMED centre ({:.1},{:.1},{:.1})",
                    mesh.name,
                    (mn[0] + mx[0]) / 2.0,
                    (mn[1] + mx[1]) / 2.0,
                    (mn[2] + mx[2]) / 2.0,
                    trans[0],
                    trans[1],
                    trans[2],
                    mscale,
                    (tmn[0] + tmx[0]) / 2.0,
                    (tmn[1] + tmx[1]) / 2.0,
                    (tmn[2] + tmx[2]) / 2.0,
                );
            }
        }
    }
    println!("\n=== I3. Exterior camera anchors (Bevy m) ===");
    for r in &ext_refs {
        if r.form_id == 0x00022469 || r.base_edid.to_lowercase().contains("mill") {
            let b = bevy(r.position);
            println!(
                "  ref 0x{:08x} {} {} Bevy=({:.1},{:.1},{:.1})",
                r.form_id, r.base_type, r.base_edid, b[0], b[1], b[2]
            );
        }
    }

    // ---- Part J: why is the head skin at the feet? Dump the
    // skin data (instance bones, GeomSkin, BoneSkin, stored
    // positions) for the head vs the mouth vs the hair.
    println!("\n=== J. FaceGen skin data (head vs mouth vs hair) ===");
    if let Some(bytes) = archives.read_file(facegen) {
        if let Ok(nif) = NifFile::from_bytes(&bytes) {
            // Node names + local translations by block index.
            let mut node_info: HashMap<i32, (String, [f32; 3])> = HashMap::new();
            for (i, b) in nif.blocks.iter().enumerate() {
                if let dovah_nif::Block::Node(n) = b {
                    node_info.insert(i as i32, (n.av.net.name.clone(), n.av.translation));
                }
            }
            for (i, b) in nif.blocks.iter().enumerate() {
                let (shape, dynamic, kind): (
                    &dovah_nif::BsTriShapeBlock,
                    &[dovah_nif::Vec3],
                    &str,
                ) = match b {
                    dovah_nif::Block::BsTriShape(s) => (s, &[], "BSTriShape"),
                    dovah_nif::Block::BsDynamicTriShape(d) => {
                        (&d.shape, &d.dynamic_positions, "BSDynamicTriShape")
                    }
                    _ => continue,
                };
                let name = &shape.av.net.name;
                if name != "MaleHeadNord"
                    && name != "MaleMouthHumanoidDefault"
                    && name != "HairMaleNord06"
                {
                    continue;
                }
                println!(
                    "  --- shape '{name}' ({kind}, block {i}) skin={} ---",
                    shape.skin
                );
                // Stored positions: dynamic section for
                // BSDynamicTriShape, else inline vertices.
                let stored: Vec<[f32; 3]> = if !dynamic.is_empty() {
                    dynamic.to_vec()
                } else {
                    shape.vertices.iter().filter_map(|v| v.position).collect()
                };
                if !stored.is_empty() {
                    let mut mn = [f32::INFINITY; 3];
                    let mut mx = [f32::NEG_INFINITY; 3];
                    for v in &stored {
                        for k in 0..3 {
                            mn[k] = mn[k].min(v[k]);
                            mx[k] = mx[k].max(v[k]);
                        }
                    }
                    println!(
                        "    stored positions n={} centre=({:.1},{:.1},{:.1}) z range [{:.1},{:.1}]",
                        stored.len(),
                        (mn[0] + mx[0]) / 2.0,
                        (mn[1] + mx[1]) / 2.0,
                        (mn[2] + mx[2]) / 2.0,
                        mn[2],
                        mx[2],
                    );
                } else {
                    println!("    stored positions: NONE");
                }
                if shape.skin < 0 {
                    println!("    no skin instance");
                    continue;
                }
                let Some(dovah_nif::Block::SkinInstance(inst)) =
                    nif.blocks.get(shape.skin as usize)
                else {
                    println!("    skin block {} is not a SkinInstance", shape.skin);
                    continue;
                };
                println!(
                    "    skin instance: data={} partition={} root={} bones={:?} dismember={}",
                    inst.data, inst.partition, inst.root, inst.bones, inst.dismember
                );
                for (bi, node_idx) in inst.bones.iter().enumerate() {
                    let (nname, trans) = node_info
                        .get(node_idx)
                        .cloned()
                        .unwrap_or_else(|| ("<not-a-node>".to_string(), [f32::NAN; 3]));
                    println!(
                        "      bone {bi}: node {node_idx} '{nname}' local trans=({:.1},{:.1},{:.1})",
                        trans[0], trans[1], trans[2]
                    );
                }
                if let Some(dovah_nif::Block::SkinData(sd)) = nif.blocks.get(inst.data as usize) {
                    let gt = sd.skin_transform.translation;
                    println!(
                        "    skin data: GeomSkin trans=({:.2},{:.2},{:.2}) scale={:.2} bones={} trailing={}",
                        gt[0],
                        gt[1],
                        gt[2],
                        sd.skin_transform.scale,
                        sd.bones.len(),
                        sd.trailing_bytes
                    );
                    for (bi, bone) in sd.bones.iter().enumerate() {
                        let bt = bone.transform.translation;
                        println!(
                            "      BoneSkin {bi}: trans=({:.2},{:.2},{:.2}) scale={:.2} weights={} sphere=({:.1},{:.1},{:.1} r={:.1})",
                            bt[0],
                            bt[1],
                            bt[2],
                            bone.transform.scale,
                            bone.weights.len(),
                            bone.bounding_sphere.0[0],
                            bone.bounding_sphere.0[1],
                            bone.bounding_sphere.0[2],
                            bone.bounding_sphere.1,
                        );
                    }
                }
                if let Some(dovah_nif::Block::SkinPartition(sp)) =
                    nif.blocks.get(inst.partition as usize)
                {
                    println!(
                        "    skin partition: verts={} vdesc=0x{:x} partitions={} trailing={}",
                        sp.vertices.len(),
                        sp.vertex_desc,
                        sp.partitions.len(),
                        sp.trailing_bytes
                    );
                    for (pi, part) in sp.partitions.iter().enumerate() {
                        let nw = part.weights.iter().filter(|w| !w.is_empty()).count();
                        println!(
                            "      part {pi}: verts={} tris={} bones={:?} weighted_verts={}",
                            part.vertex_map.len(),
                            part.triangles.len(),
                            part.bones,
                            nw,
                        );
                        // First non-empty weight list.
                        if let Some(w) = part.weights.iter().find(|w| !w.is_empty()) {
                            println!("        first weights: {w:?}");
                        }
                    }
                }
            }
        }
    }
    // ---- Part K1: FaceGen head AS THE RENDERER SEES IT.
    // The counter render still shows Orgnar's face as a black
    // void, but Part H proved the skin's winding-derived
    // normals are 81% outward — on RAW vertices. convert.rs
    // applies mesh.transform FIRST, then computes normals. If
    // the transform mirrors (det -1), the rendered normals
    // point inward. Also: are the skin's vertex colors dark?
    // (G4 never printed them — dark vcolors blacken the face
    // regardless of normals.)
    println!("\n=== K1. FaceGen as the renderer sees it ===");
    for (label, fg) in [
        ("Orgnar", facegen),
        (
            "Delphine",
            "meshes\\actors\\character\\facegendata\\facegeom\\skyrim.esm\\00013478.nif",
        ),
    ] {
        let Some(bytes) = archives.read_file(fg) else {
            println!("  {label}: NIF NOT FOUND");
            continue;
        };
        let Ok(nif) = NifFile::from_bytes(&bytes) else {
            println!("  {label}: PARSE FAILED");
            continue;
        };
        for mesh in nif.meshes() {
            if mesh.vertices.is_empty() {
                continue;
            }
            let (nc, vmean, vmin) = vcolor_stats(&mesh.vertex_colors);
            // Transform determinant: +1 = rotation, -1 = mirror.
            let r = mesh.transform.rotation;
            let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1])
                - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
                + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
            // Transformed vertices (convert.rs order: rot * v *
            // scale + trans), then Bevy-convention smooth
            // normals, tested outward vs the transformed centre.
            let tverts: Vec<[f32; 3]> = mesh
                .vertices
                .iter()
                .map(|v| {
                    let q = mat_vec(&r, *v);
                    let s = mesh.transform.scale;
                    let t = mesh.transform.translation;
                    [q[0] * s + t[0], q[1] * s + t[1], q[2] * s + t[2]]
                })
                .collect();
            let mut acc = vec![[0.0f32; 3]; tverts.len()];
            for tri in mesh.indices.chunks(3) {
                if tri.len() < 3 {
                    continue;
                }
                let (a, b, c) = (
                    tverts[tri[0] as usize],
                    tverts[tri[1] as usize],
                    tverts[tri[2] as usize],
                );
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let f = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                for idx in tri {
                    let s = &mut acc[*idx as usize];
                    s[0] += f[0];
                    s[1] += f[1];
                    s[2] += f[2];
                }
            }
            let mut centre = [0.0f32; 3];
            for p in &tverts {
                centre[0] += p[0];
                centre[1] += p[1];
                centre[2] += p[2];
            }
            let n = tverts.len() as f32;
            centre = [centre[0] / n, centre[1] / n, centre[2] / n];
            let mut outward = 0usize;
            let mut mean_dot = 0.0f32;
            let mut counted = 0usize;
            let mut stored_dot = 0.0f32;
            let mut stored_n = 0usize;
            for (i, p) in tverts.iter().enumerate() {
                let a = acc[i];
                let len = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
                if len < 1e-9 {
                    continue;
                }
                let cn = [a[0] / len, a[1] / len, a[2] / len];
                let d = [p[0] - centre[0], p[1] - centre[1], p[2] - centre[2]];
                let dl = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if dl < 1e-6 {
                    continue;
                }
                let dot = cn[0] * d[0] / dl + cn[1] * d[1] / dl + cn[2] * d[2] / dl;
                mean_dot += dot;
                counted += 1;
                if dot > 0.0 {
                    outward += 1;
                }
                // Stored normals, rotated by the same transform
                // (convert.rs rotates stored normals by rot).
                if i < mesh.normals.len() {
                    let s = mat_vec(&r, mesh.normals[i]);
                    let sl = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
                    if sl > 1e-6 {
                        stored_dot += cn[0] * s[0] / sl + cn[1] * s[1] / sl + cn[2] * s[2] / sl;
                        stored_n += 1;
                    }
                }
            }
            println!(
                "  {label} '{}': verts={} stored_normals={} det={:+.2} | vcolors n={} mean=({:.2},{:.2},{:.2}) min=({:.2},{:.2},{:.2}) | TRANSFORMED winding outward {:.0}% mean dot {:.2} | computed-vs-stored {:.2} (n={})",
                mesh.name,
                mesh.vertices.len(),
                mesh.normals.len(),
                det,
                nc,
                vmean[0],
                vmean[1],
                vmean[2],
                vmin[0],
                vmin[1],
                vmin[2],
                if counted > 0 {
                    100.0 * outward as f32 / counted as f32
                } else {
                    f32::NAN
                },
                if counted > 0 {
                    mean_dot / counted as f32
                } else {
                    f32::NAN
                },
                if stored_n > 0 {
                    stored_dot / stored_n as f32
                } else {
                    f32::NAN
                },
                stored_n,
            );
        }
    }

    // ---- Part K2: the red pole. Pixel scan of v047-ext-spawn:
    // 230 red pixels at x 369-372, y 457-526 — identical
    // pre/post the editor fix, so it is NOT editor geometry.
    // Census: tall-thin shapes (height > 2 m, > 3x the other
    // axes) with their vertex colors + diffuse, plus the G3
    // rogue-triangle shapes (ropes, TreePineForest05) by name.
    println!("\n=== K2. Red pole: tall-thin shapes + rogue shapes ===");
    let mut k2 = 0usize;
    for pref in &ext_refs {
        let entry = nif_cache.entry(pref.model_path.clone()).or_insert_with(|| {
            archives
                .read_file(&pref.model_path)
                .and_then(|b| NifFile::from_bytes(&b).ok())
        });
        let Some(nif) = entry else { continue };
        for mesh in nif.meshes() {
            if mesh.vertices.is_empty() {
                continue;
            }
            let mut mn = [f32::INFINITY; 3];
            let mut mx = [f32::NEG_INFINITY; 3];
            for v in &mesh.vertices {
                for i in 0..3 {
                    mn[i] = mn[i].min(v[i]);
                    mx[i] = mx[i].max(v[i]);
                }
            }
            let size = [
                (mx[0] - mn[0]) * pref.scale / 70.0,
                (mx[1] - mn[1]) * pref.scale / 70.0,
                (mx[2] - mn[2]) * pref.scale / 70.0,
            ];
            let height = size[2];
            let thin = height > 2.0 && height > 3.0 * size[0].max(size[1]);
            let is_rogue = mesh.name.contains("Rope") || mesh.name.contains("PineForest05");
            if !thin && !is_rogue {
                continue;
            }
            // World centre: mesh transform, then ref rotation +
            // scale + translation, then Skyrim -> Bevy.
            let lc = [
                (mn[0] + mx[0]) / 2.0,
                (mn[1] + mx[1]) / 2.0,
                (mn[2] + mx[2]) / 2.0,
            ];
            let tq = mat_vec(&mesh.transform.rotation, lc);
            let ts = mesh.transform.scale;
            let tt = mesh.transform.translation;
            let local = [tq[0] * ts + tt[0], tq[1] * ts + tt[1], tq[2] * ts + tt[2]];
            let rr = ref_rotation(pref.rotation);
            let rw = mat_vec(&rr, local);
            let world = [
                rw[0] * pref.scale + pref.position[0],
                rw[1] * pref.scale + pref.position[1],
                rw[2] * pref.scale + pref.position[2],
            ];
            let b = bevy(world);
            let (nc, vmean, _) = vcolor_stats(&mesh.vertex_colors);
            let diff = mesh.textures.diffuse.as_deref().unwrap_or("<none>");
            println!(
                "  ref 0x{:08x} {} shape '{}' size=({:.2},{:.2},{:.2})m centre Bevy=({:.1},{:.1},{:.1}) vcolors n={} mean=({:.2},{:.2},{:.2}) diffuse={}",
                pref.form_id,
                pref.base_edid,
                mesh.name,
                size[0],
                size[1],
                size[2],
                b[0],
                b[1],
                b[2],
                nc,
                vmean[0],
                vmean[1],
                vmean[2],
                diff,
            );
            k2 += 1;
        }
    }
    println!("  K2 total: {k2}");

    // ---- Part K3: the counter bar top. The counter render
    // shows the top darker than the front panel though it
    // faces the overhead light. Find the counter refs near
    // (-17.3, 0, -0.5) Bevy and dump their shapes' normals +
    // vcolors (top shape: normals up or down? dark vcolors?).
    println!("\n=== K3. Counter bar refs + shapes ===");
    let mut counter_nifs: Vec<String> = Vec::new();
    for pref in &inn_refs {
        let b = bevy(pref.position);
        let d = ((b[0] + 17.33).powi(2) + b[1].powi(2) + (b[2] + 0.5).powi(2)).sqrt();
        if d > 4.0 {
            continue;
        }
        println!(
            "  ref 0x{:08x} {} {} Bevy=({:.2},{:.2},{:.2}) model={}",
            pref.form_id, pref.base_type, pref.base_edid, b[0], b[1], b[2], pref.model_path,
        );
        if pref.base_edid.to_lowercase().contains("counter")
            || pref.base_edid.to_lowercase().contains("bar")
        {
            counter_nifs.push(pref.model_path.clone());
        }
    }
    counter_nifs.sort();
    counter_nifs.dedup();
    for path in &counter_nifs {
        dump_nif_materials(&mut archives, path, "counter");
        if let Some(bytes) = archives.read_file(path) {
            if let Ok(nif) = NifFile::from_bytes(&bytes) {
                for mesh in nif.meshes() {
                    if mesh.normals.is_empty() {
                        println!("    shape '{}': NO NORMALS", mesh.name);
                        continue;
                    }
                    let mut mean = [0.0f32; 3];
                    let mut up = 0usize;
                    let mut down = 0usize;
                    for n in &mesh.normals {
                        mean[0] += n[0];
                        mean[1] += n[1];
                        mean[2] += n[2];
                        if n[2] > 0.5 {
                            up += 1;
                        } else if n[2] < -0.5 {
                            down += 1;
                        }
                    }
                    let n = mesh.normals.len() as f32;
                    println!(
                        "    shape '{}': mean normal ({:.2},{:.2},{:.2}) up {:.0}% down {:.0}%",
                        mesh.name,
                        mean[0] / n,
                        mean[1] / n,
                        mean[2] / n,
                        100.0 * up as f32 / n,
                        100.0 * down as f32 / n,
                    );
                }
            }
        }
    }

    // ---- Part L: real fog geometry? The white wash survives
    // the editor-marker fix (ext-spawn street band identical
    // 82/82 pre/post), so it is not marker volumes. Check
    // whether any ref in the block uses a fog/beam/mist NIF
    // with REAL (non-marker) shapes — unlit blended white
    // planes would wash the mid-ground. (Mist NIFs checked in
    // Part D held ONLY an EditorMarker; the fxambbeamslowfog*
    // / fxfog* set from G1 was never checked for refs.)
    println!("\n=== L. Fog/beam/mist refs in the block ===");
    let mut fog_refs = 0usize;
    for pref in &ext_refs {
        let m = pref.model_path.to_lowercase();
        if !(m.contains("fog") || m.contains("beam") || m.contains("mist")) {
            continue;
        }
        fog_refs += 1;
        println!(
            "  ref 0x{:08x} {} {} model={}",
            pref.form_id, pref.base_type, pref.base_edid, pref.model_path
        );
        dump_nif_materials(&mut archives, &pref.model_path, &pref.base_edid);
    }
    println!("  fog/beam/mist refs in block: {fog_refs}");

    // ---- Part M: the 1 missing exterior texture. The render
    // log prints it as `^HERR` (0x08 + "ERR") — a garbage
    // path, not a real texture name. One missing texture can
    // still wash a wide area: materials are cached by texture,
    // so a shared road/terrain diffuse renders grey everywhere
    // while counting once. Census every unresolvable diffuse
    // (and gradient/normal) path in the block, with the NIF +
    // shape + ref that uses it.
    println!("\n=== M. Unresolvable texture paths in the block ===");
    let mut m_bad = 0usize;
    for refs in [&ext_refs, &inn_refs] {
        for pref in refs.iter() {
            let entry = nif_cache.entry(pref.model_path.clone()).or_insert_with(|| {
                archives
                    .read_file(&pref.model_path)
                    .and_then(|b| NifFile::from_bytes(&b).ok())
            });
            let Some(nif) = entry else { continue };
            for mesh in nif.meshes() {
                for (slot, path) in [
                    ("diffuse", &mesh.textures.diffuse),
                    ("gradient", &mesh.textures.gradient),
                    ("normal", &mesh.textures.normal),
                ] {
                    let Some(p) = path else { continue };
                    let control = p.bytes().any(|b| b < 0x20 || b == 0x7f);
                    if control || !archives.has_file(p) {
                        m_bad += 1;
                        println!(
                            "  ref 0x{:08x} {} | NIF {} | shape '{}' | {slot}={:?} resolved={}",
                            pref.form_id,
                            pref.base_edid,
                            pref.model_path,
                            mesh.name,
                            p,
                            archives.has_file(p),
                        );
                    }
                }
            }
        }
    }
    println!("  unresolvable texture slots in block: {m_bad}");

    // ---- Part N: are the source textures grey or brown?
    // The wash is desaturation, not clipping — the last
    // distinguishing test: decode the road/cliff/grass/wood
    // diffuse DDS files and print their mean sRGB. If the
    // road texture is brown but renders (80,80,75) grey, the
    // pipeline (tonemap/exposure) desaturates it; if the
    // texture itself is grey, the render is faithful.
    println!("\n=== N. Source texture mean colors ===");
    let mut tex_paths: Vec<String> = Vec::new();
    for pref in &ext_refs {
        if !pref.base_edid.contains("RoadChunk") {
            continue;
        }
        let entry = nif_cache.entry(pref.model_path.clone()).or_insert_with(|| {
            archives
                .read_file(&pref.model_path)
                .and_then(|b| NifFile::from_bytes(&b).ok())
        });
        let Some(nif) = entry else { continue };
        for mesh in nif.meshes() {
            if let Some(d) = &mesh.textures.diffuse {
                tex_paths.push(d.clone());
            }
        }
    }
    // Controls: textures whose rendered color is known.
    tex_paths.push("textures\\landscape\\dirtcliffs\\DirtCliffs01.dds".to_string());
    tex_paths.push("textures\\landscape\\FieldGrass02.dds".to_string());
    tex_paths.push("textures\\architecture\\farmhouse\\WoodPost02.dds".to_string());
    tex_paths.push("textures\\landscape\\trees\\TreePineForestBarkComp.dds".to_string());
    tex_paths.sort();
    tex_paths.dedup();
    for path in &tex_paths {
        match archives.read_file(path) {
            Some(bytes) => match dovah_dds::DdsImage::from_bytes(&bytes) {
                Ok(dds) => match dds.decode(0) {
                    Ok(img) => {
                        let n = (img.width * img.height) as f64;
                        let mut sr = 0.0f64;
                        let mut sg = 0.0f64;
                        let mut sb = 0.0f64;
                        for px in img.data.chunks(4) {
                            sr += px[0] as f64;
                            sg += px[1] as f64;
                            sb += px[2] as f64;
                        }
                        println!(
                            "  {} ({}x{}): mean sRGB=({:.0},{:.0},{:.0})",
                            path,
                            img.width,
                            img.height,
                            sr / n,
                            sg / n,
                            sb / n
                        );
                    }
                    Err(e) => println!("  {path}: decode failed: {e}"),
                },
                Err(e) => println!("  {path}: DDS parse failed: {e}"),
            },
            None => println!("  {path}: NOT FOUND"),
        }
    }
}

/// Dump per-shape material facts for one NIF: textures (resolved?),
/// vertex-color stats, alpha flags, unlit, skinned.
fn dump_nif_materials(archives: &mut Archives, model_path: &str, edid: &str) {
    println!("  NIF {} ({edid}):", model_path);
    let Some(bytes) = archives.read_file(model_path) else {
        println!("    NIF NOT FOUND");
        return;
    };
    let Ok(nif) = NifFile::from_bytes(&bytes) else {
        println!("    NIF PARSE FAILED");
        return;
    };
    for mesh in nif.meshes() {
        if mesh.vertices.is_empty() {
            continue;
        }
        let (nc, vmean, vmin) = vcolor_stats(&mesh.vertex_colors);
        let diff = mesh.textures.diffuse.as_deref().unwrap_or("<none>");
        let diff_ok = mesh
            .textures
            .diffuse
            .as_ref()
            .map(|p| archives.has_file(p))
            .unwrap_or(false);
        let grad = mesh.textures.gradient.as_deref().unwrap_or("<none>");
        println!(
            "    shape '{}': {} verts skinned={} unlit={} blend={:?} cutoff={:?}",
            mesh.name,
            mesh.vertices.len(),
            mesh.skinned,
            mesh.unlit,
            mesh.alpha_blend,
            mesh.alpha_cutoff
        );
        println!(
            "      diffuse={} resolved={} gradient={} | vcolors n={} mean=({:.2},{:.2},{:.2},{:.2}) min=({:.2},{:.2},{:.2},{:.2})",
            diff,
            diff_ok,
            grad,
            nc,
            vmean[0],
            vmean[1],
            vmean[2],
            vmean[3],
            vmin[0],
            vmin[1],
            vmin[2],
            vmin[3],
        );
    }
}
