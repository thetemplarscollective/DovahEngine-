//! M11 probe: audit the Riverwood exterior against the data.
//!
//! Answers four questions with numbers:
//! A. TERRAIN vs BUILDINGS: for every placed object with a model,
//!    the world-space bottom of its mesh (REFR transform applied,
//!    mirroring the app's render state: shape-local transforms
//!    only, no parent composition) minus the terrain height at
//!    its footprint. A systematic negative offset means the
//!    ground buries the buildings.
//! B. PARENT TRANSFORMS: for every NIF used in the exterior (and
//!    the inn), whether any shape hangs under a non-identity
//!    parent NiNode transform — the app ignores parent transforms
//!    (convert.rs), so a non-identity parent means the mesh
//!    renders in the wrong place. This is the prime suspect for
//!    both "buildings not in the right spots" and the deferred
//!    door-panel bug.
//! C. ALPHA: which shapes carry a NiAlphaProperty (flags +
//!    threshold), and — for the tree textures — the fraction of
//!    transparent pixels in the diffuse DDS. Opaque rendering of
//!    a mostly-transparent needle texture is papier-mâché.
//! D. DOOR vs TERRAIN: the inn's exterior door ref and the XTEL
//!    arrival point against the terrain height.
//! E. DOOR PANELS: each door ref's panel mesh in world space
//!    (AABB + size in metres + bottom vs ref Z) — the deferred
//!    door-position bug, measured instead of guessed.
//! F. TERRAIN vs REFR Z (M13): for EVERY placed ref in the 3×3
//!    (model or not), terrain height minus ref placement Z —
//!    re-proves the ×8 height scale with the shipped decode
//!    (research audit: ×8 median −0.03 m, 60% within 0.5 m).
//!
//! Usage: cargo run -p dovah-app --example exterior_audit

use dovah_bsa::BsaArchive;
use dovah_esm::{Group, GroupType, Plugin, Record};
use dovah_nif::{Block, NifFile};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::Path;

const DATA_DIR: &str = "test-data";
const ESM_PATH: &str = "test-data/Skyrim.esm";
const CENTRE: (i32, i32) = (5, -11);
const CELL_SPAN: f32 = 4096.0;
const PERSISTENT_CELL_FORM_ID: u32 = 0x00000d74;
const TAMRIEL_WRLD: u32 = 0x0000003c;
const INN_EDID: &str = "RiverwoodSleepingGiantInn";

const FLAG_INITIALLY_DISABLED: u32 = 0x00000800;
const FLAG_IS_MARKER: u32 = 0x00000020;

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

/// Tamriel's exterior cells by grid, from the WorldChildren group
/// (mirrors cell.rs TamrielCells).
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

// ---- LAND decode (mirrors land.rs: mixed accumulation) ----

const LAND_SIZE: usize = 33;

struct Land {
    heights: [f32; LAND_SIZE * LAND_SIZE],
}

fn decode_land(record: &Record) -> Option<Land> {
    let vhgt = record.field(b"VHGT")?;
    if vhgt.data.len() != 1096 {
        return None;
    }
    // M13: mirrors land.rs — accumulated height ×8 (anchor
    // included) to get game units. Keep in sync with
    // `land::HEIGHT_SCALE`.
    let first = f32::from_le_bytes(vhgt.data[0..4].try_into().unwrap()) * 8.0;
    let d = |i: usize| vhgt.data[4 + i] as i8 as f32 * 8.0;
    let mut heights = [0.0f32; LAND_SIZE * LAND_SIZE];
    let mut col_acc = first;
    for y in 0..LAND_SIZE {
        col_acc += d(y * LAND_SIZE);
        let mut row_acc = col_acc;
        heights[y * LAND_SIZE] = row_acc;
        for x in 1..LAND_SIZE {
            row_acc += d(y * LAND_SIZE + x);
            heights[y * LAND_SIZE + x] = row_acc;
        }
    }
    Some(Land { heights })
}

impl Land {
    /// Bilinear sample at cell-local coords (0..4096, west->east,
    /// south->north). Mirrors land.rs `sample_height`.
    fn sample(&self, lx: f32, ly: f32) -> Option<f32> {
        let fx = lx / 128.0;
        let fy = ly / 128.0;
        if !(0.0..=32.0).contains(&fx) || !(0.0..=32.0).contains(&fy) {
            return None;
        }
        let x0 = (fx.floor() as usize).min(31);
        let y0 = (fy.floor() as usize).min(31);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let h00 = self.heights[y0 * LAND_SIZE + x0];
        let h10 = self.heights[y0 * LAND_SIZE + x0 + 1];
        let h01 = self.heights[(y0 + 1) * LAND_SIZE + x0];
        let h11 = self.heights[(y0 + 1) * LAND_SIZE + x0 + 1];
        Some(
            h00 * (1.0 - tx) * (1.0 - ty)
                + h10 * tx * (1.0 - ty)
                + h01 * (1.0 - tx) * ty
                + h11 * tx * ty,
        )
    }
}

// ---- small row-major 3x3 matrix math (dovah_nif Mat3 layout) ----

type M3 = [[f32; 3]; 3];

fn mat_mul(a: &M3, b: &M3) -> M3 {
    let mut out = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}

fn mat_vec(m: &M3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
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

/// REFR Euler composition, mirroring coords.rs: Rz * Ry * Rx.
fn ref_rotation(rot: [f32; 3]) -> M3 {
    mat_mul(&mat_mul(&rot_z(rot[2]), &rot_y(rot[1])), &rot_x(rot[0]))
}

/// Rotation angle (radians) of a rotation matrix, via the trace.
fn rot_angle(m: &M3) -> f32 {
    let trace = m[0][0] + m[1][1] + m[2][2];
    ((trace - 1.0) / 2.0).clamp(-1.0, 1.0).acos()
}

fn identity() -> M3 {
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
}

/// One placed ref the app would draw (filters mirror
/// cell.rs `append_objects`, minus the effects skip).
struct PlacedRef {
    base_type: String,
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
            continue; // actors are capsules, no mesh
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
            base_type: base.type_str(),
            base_edid: base.edid().unwrap_or_default(),
            model_path: format!("meshes\\{model}"),
            position,
            rotation,
            scale,
        });
    }
}

/// Per-NIF local bounds (Skyrim units), after each shape's LOCAL
/// transform only — exactly what the app renders today.
struct NifBounds {
    /// (min, max) per drawable sub-mesh, local space.
    meshes: Vec<([f32; 3], [f32; 3])>,
}

fn nif_bounds(nif: &NifFile) -> NifBounds {
    let mut meshes = Vec::new();
    for mesh in nif.meshes() {
        if mesh.vertices.is_empty() || mesh.indices.is_empty() {
            continue;
        }
        let rot = mesh.transform.rotation;
        let scale = mesh.transform.scale;
        let trans = mesh.transform.translation;
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for v in &mesh.vertices {
            // Mirror convert.rs: rot * v * scale + trans.
            let r = mat_vec(&rot, *v);
            let p = [
                r[0] * scale + trans[0],
                r[1] * scale + trans[1],
                r[2] * scale + trans[2],
            ];
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        meshes.push((min, max));
    }
    NifBounds { meshes }
}

/// World-space AABB of a local AABB under a REFR transform.
/// All 8 corners are transformed (exact for yaw-only rotations,
/// which is nearly every placed object).
fn world_aabb(
    local: ([f32; 3], [f32; 3]),
    position: [f32; 3],
    rotation: [f32; 3],
    scale: f32,
) -> ([f32; 3], [f32; 3]) {
    let (min, max) = local;
    let rot = ref_rotation(rotation);
    let mut wmin = [f32::INFINITY; 3];
    let mut wmax = [f32::NEG_INFINITY; 3];
    for i in 0..8 {
        let corner = [
            if i & 1 == 0 { min[0] } else { max[0] },
            if i & 2 == 0 { min[1] } else { max[1] },
            if i & 4 == 0 { min[2] } else { max[2] },
        ];
        let r = mat_vec(
            &rot,
            [corner[0] * scale, corner[1] * scale, corner[2] * scale],
        );
        for axis in 0..3 {
            let v = r[axis] + position[axis];
            wmin[axis] = wmin[axis].min(v);
            wmax[axis] = wmax[axis].max(v);
        }
    }
    (wmin, wmax)
}

/// A door reference: form ID retained, so its panel can be
/// matched against the render.
struct DoorRef {
    form_id: u32,
    base_edid: String,
    model_path: String,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: f32,
}

fn collect_doors(records: &[&Record], by_form_id: &HashMap<u32, &Record>, out: &mut Vec<DoorRef>) {
    for r in records {
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
        let Some(base_id) = form_id_from_field(r, b"NAME") else {
            continue;
        };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        if base.type_str() != "DOOR" {
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
        out.push(DoorRef {
            form_id: r.form_id,
            base_edid: base.edid().unwrap_or_default(),
            model_path: format!("meshes\\{model}"),
            position,
            rotation,
            scale,
        });
    }
}

/// World-space min Z of a mesh AABB under a REFR transform.
/// The 8 corners are transformed (exact for yaw-only rotations,
/// which is nearly every placed object).
fn world_bottom(
    local: ([f32; 3], [f32; 3]),
    position: [f32; 3],
    rotation: [f32; 3],
    scale: f32,
) -> f32 {
    let (min, max) = local;
    let rot = ref_rotation(rotation);
    let mut bottom = f32::INFINITY;
    for i in 0..8 {
        let corner = [
            if i & 1 == 0 { min[0] } else { max[0] },
            if i & 2 == 0 { min[1] } else { max[1] },
            if i & 4 == 0 { min[2] } else { max[2] },
        ];
        let r = mat_vec(
            &rot,
            [corner[0] * scale, corner[1] * scale, corner[2] * scale],
        );
        let z = r[2] + position[2];
        bottom = bottom.min(z);
    }
    bottom
}

/// A shape's composed parent transform (everything above the shape
/// in the node tree, excluding the shape's own local transform).
#[derive(Clone, Copy)]
struct Composed {
    translation: [f32; 3],
    rotation: M3,
    scale: f32,
}

impl Composed {
    fn identity() -> Self {
        Composed {
            translation: [0.0, 0.0, 0.0],
            rotation: identity(),
            scale: 1.0,
        }
    }

    /// Compose a child AvObject transform UNDER this one:
    /// result = self ∘ child.
    fn compose(&self, t: [f32; 3], r: &M3, s: f32) -> Self {
        let scaled = [t[0] * self.scale, t[1] * self.scale, t[2] * self.scale];
        let rt = mat_vec(&self.rotation, scaled);
        Composed {
            translation: [
                self.translation[0] + rt[0],
                self.translation[1] + rt[1],
                self.translation[2] + rt[2],
            ],
            rotation: mat_mul(&self.rotation, r),
            scale: self.scale * s,
        }
    }

    fn is_identity(&self) -> bool {
        let t = (self.translation[0].powi(2)
            + self.translation[1].powi(2)
            + self.translation[2].powi(2))
        .sqrt();
        t < 0.5 && rot_angle(&self.rotation) < 0.001 && (self.scale - 1.0).abs() < 0.001
    }
}

/// Part B: for one NIF, find shapes whose composed PARENT
/// transform is non-identity. Returns (shape name, parent
/// translation length, parent rotation angle, parent scale).
fn parent_audit(nif: &NifFile) -> Vec<(String, f32, f32, f32)> {
    let mut out = Vec::new();
    // Roots: footer roots, else block 0.
    let roots: Vec<i32> = if nif.root_indices.is_empty() {
        vec![0]
    } else {
        nif.root_indices.clone()
    };
    // Stack: (block index, composed transform of everything ABOVE
    // this block). A block's own transform composes under its
    // parent's when descending to its children.
    let mut stack: Vec<(i32, Composed)> = roots
        .into_iter()
        .map(|r| (r, Composed::identity()))
        .collect();
    let mut visited: HashSet<i32> = HashSet::new();
    while let Some((idx, parent)) = stack.pop() {
        if !visited.insert(idx) {
            continue;
        }
        let Some(block) = nif.block(idx) else {
            continue;
        };
        match block {
            Block::Node(node) => {
                let composed =
                    parent.compose(node.av.translation, &node.av.rotation, node.av.scale);
                for child in &node.children {
                    stack.push((*child, composed));
                }
            }
            // M13: audit walks THROUGH switches into every child
            // (it measures transforms, it does not render).
            Block::SwitchNode(switch) => {
                let node = &switch.node;
                let composed =
                    parent.compose(node.av.translation, &node.av.rotation, node.av.scale);
                for child in &node.children {
                    stack.push((*child, composed));
                }
            }
            Block::TriShape(s) => check_shape(&s.geometry, &parent, &mut out),
            Block::TriStrips(s) => check_shape(&s.geometry, &parent, &mut out),
            Block::BsLodTriShape(s) => check_shape(&s.geometry, &parent, &mut out),
            Block::BsTriShape(s) => {
                check_shape_av(&s.av, &parent, &mut out);
            }
            Block::BsDynamicTriShape(d) => {
                check_shape_av(&d.shape.av, &parent, &mut out);
            }
            _ => {}
        }
    }
    out
}

fn check_shape(
    geometry: &dovah_nif::Geometry,
    parent: &Composed,
    out: &mut Vec<(String, f32, f32, f32)>,
) {
    check_shape_av(&geometry.av, parent, out);
}

fn check_shape_av(
    av: &dovah_nif::AvObject,
    parent: &Composed,
    out: &mut Vec<(String, f32, f32, f32)>,
) {
    if parent.is_identity() {
        return;
    }
    let t = (parent.translation[0].powi(2)
        + parent.translation[1].powi(2)
        + parent.translation[2].powi(2))
    .sqrt();
    out.push((
        av.net.name.clone(),
        t,
        rot_angle(&parent.rotation),
        parent.scale,
    ));
}

/// Part C: NiAlphaProperty audit for one NIF.
/// Returns (shape name, flags, threshold, diffuse path).
fn alpha_audit(nif: &NifFile) -> Vec<(String, u16, u8, Option<String>)> {
    let mut out = Vec::new();
    for block in &nif.blocks {
        let (name, alpha_idx, shader_idx) = match block {
            Block::TriShape(s) => (
                s.geometry.av.net.name.clone(),
                s.geometry.alpha_property,
                s.geometry.shader_property,
            ),
            Block::TriStrips(s) => (
                s.geometry.av.net.name.clone(),
                s.geometry.alpha_property,
                s.geometry.shader_property,
            ),
            Block::BsLodTriShape(s) => (
                s.geometry.av.net.name.clone(),
                s.geometry.alpha_property,
                s.geometry.shader_property,
            ),
            Block::BsTriShape(s) => (s.av.net.name.clone(), s.alpha_property, s.shader_property),
            Block::BsDynamicTriShape(d) => (
                d.shape.av.net.name.clone(),
                d.shape.alpha_property,
                d.shape.shader_property,
            ),
            _ => continue,
        };
        if alpha_idx < 0 {
            continue;
        }
        let Some(Block::AlphaProperty(prop)) = nif.block(alpha_idx) else {
            continue;
        };
        // Diffuse path via the shader's texture set (slot 0).
        let mut diffuse = None;
        if let Some(Block::LightingShaderProperty(shader)) = nif.block(shader_idx) {
            if let Some(Block::ShaderTextureSet(set)) = nif.block(shader.texture_set) {
                diffuse = set.textures.first().filter(|s| !s.is_empty()).cloned();
            }
        }
        out.push((name, prop.flags, prop.threshold, diffuse));
    }
    out
}

fn median(mut values: Vec<f32>) -> f32 {
    if values.is_empty() {
        return f32::NAN;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    values[values.len() / 2]
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let index = CellIndex::build(&plugin);
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    let tamriel = tamriel_cells(&plugin);
    println!("Tamriel grid map: {} cells", tamriel.len());

    // ---- collect the 3x3 refs + LANDs ----
    let mut refs: Vec<PlacedRef> = Vec::new();
    let mut lands: HashMap<(i32, i32), Land> = HashMap::new();
    for gx in CENTRE.0 - 1..=CENTRE.0 + 1 {
        for gy in CENTRE.1 - 1..=CENTRE.1 + 1 {
            let Some(cell) = tamriel.get(&(gx, gy)) else {
                println!("  grid ({gx},{gy}): NO CELL");
                continue;
            };
            let children = index.children_of(cell.form_id);
            if let Some(land) = children.iter().find(|r| r.type_str() == "LAND") {
                if let Some(decoded) = decode_land(land) {
                    lands.insert((gx, gy), decoded);
                }
                // DATA field value, for the offset question.
                if let Some(f) = land.field(b"DATA") {
                    let v = i32::from_le_bytes(f.data[0..4].try_into().unwrap());
                    println!("  grid ({gx},{gy}): LAND DATA={v}");
                }
            }
            collect_refs(children, &by_form_id, &mut refs);
        }
    }
    // Persistent refs inside the block.
    let min_x = (CENTRE.0 - 1) as f32 * CELL_SPAN;
    let max_x = (CENTRE.0 + 2) as f32 * CELL_SPAN;
    let min_y = (CENTRE.1 - 1) as f32 * CELL_SPAN;
    let max_y = (CENTRE.1 + 2) as f32 * CELL_SPAN;
    let persist: Vec<&Record> = index
        .children_of(PERSISTENT_CELL_FORM_ID)
        .iter()
        .copied()
        .filter(|r| {
            (r.type_str() == "REFR" || r.type_str() == "ACHR")
                && r.field(b"DATA")
                    .and_then(|f| parse_placement(&f.data))
                    .map(|(pos, _)| {
                        pos[0] >= min_x && pos[0] < max_x && pos[1] >= min_y && pos[1] < max_y
                    })
                    .unwrap_or(false)
        })
        .collect();
    collect_refs(&persist, &by_form_id, &mut refs);
    println!(
        "exterior refs with models: {} (lands: {}/9)",
        refs.len(),
        lands.len()
    );

    // Inn refs too (for the parent audit — the door lives there).
    let mut inn_refs: Vec<PlacedRef> = Vec::new();
    let mut door_refs: Vec<DoorRef> = Vec::new();
    if let Some(inn) = plugin
        .all_records()
        .into_iter()
        .find(|r| r.type_str() == "CELL" && r.edid().as_deref() == Some(INN_EDID))
    {
        collect_refs(index.children_of(inn.form_id), &by_form_id, &mut inn_refs);
        collect_doors(index.children_of(inn.form_id), &by_form_id, &mut door_refs);
    }
    println!("inn refs with models: {}", inn_refs.len());
    // Exterior door refs (persistent, inside the block).
    collect_doors(&persist, &by_form_id, &mut door_refs);
    println!("door refs (inn + exterior block): {}", door_refs.len());

    let terrain_at = |x: f32, y: f32| -> Option<f32> {
        let gx = (x / CELL_SPAN).floor() as i32;
        let gy = (y / CELL_SPAN).floor() as i32;
        let land = lands.get(&(gx, gy))?;
        land.sample(x - gx as f32 * CELL_SPAN, y - gy as f32 * CELL_SPAN)
    };

    let mut archives = Archives::open_all(Path::new(DATA_DIR));

    // ---- Part A: terrain vs mesh bottoms ----
    println!(
        "\n=== A. TERRAIN vs MESH BOTTOMS (bottom_z - terrain_z, Skyrim units; 70 units = 1 m) ==="
    );
    let mut nif_cache: HashMap<String, Option<NifBounds>> = HashMap::new();
    let mut by_type: HashMap<String, Vec<f32>> = HashMap::new();
    let mut worst: Vec<(f32, String, String, String)> = Vec::new(); // (diff, type, edid, model)
    let mut floating: Vec<(f32, String, String, String)> = Vec::new();
    let mut no_terrain = 0usize;
    let mut no_nif = 0usize;
    for r in &refs {
        let Some(terrain) = terrain_at(r.position[0], r.position[1]) else {
            no_terrain += 1;
            continue;
        };
        let bounds = nif_cache.entry(r.model_path.clone()).or_insert_with(|| {
            archives
                .read_file(&r.model_path)
                .and_then(|bytes| NifFile::from_bytes(&bytes).ok())
                .map(|nif| nif_bounds(&nif))
        });
        let Some(bounds) = bounds else {
            no_nif += 1;
            continue;
        };
        if bounds.meshes.is_empty() {
            continue;
        }
        let mut bottom = f32::INFINITY;
        for local in &bounds.meshes {
            bottom = bottom.min(world_bottom(*local, r.position, r.rotation, r.scale));
        }
        let diff = bottom - terrain;
        by_type.entry(r.base_type.clone()).or_default().push(diff);
        worst.push((
            diff,
            r.base_type.clone(),
            r.base_edid.clone(),
            r.model_path.clone(),
        ));
        floating.push((
            diff,
            r.base_type.clone(),
            r.base_edid.clone(),
            r.model_path.clone(),
        ));
    }
    let mut types: Vec<(&String, &Vec<f32>)> = by_type.iter().collect();
    types.sort_by_key(|(t, _)| t.to_string());
    for (t, diffs) in types {
        let min = diffs.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = diffs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        println!(
            "  {:<6} n={:>4} median {:>7.1}u ({:>5.2} m)  min {:>7.1}u  max {:>7.1}u",
            t,
            diffs.len(),
            median(diffs.clone()),
            median(diffs.clone()) / 70.0,
            min,
            max
        );
    }
    worst.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    println!("  -- 20 most BURIED (mesh bottom below terrain) --");
    for (diff, t, edid, model) in worst.iter().take(20) {
        println!(
            "    {diff:>8.1}u ({:>6.2} m)  {t:<6} {edid}  {model}",
            diff / 70.0
        );
    }
    floating.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    println!("  -- 10 most FLOATING (mesh bottom above terrain) --");
    for (diff, t, edid, model) in floating.iter().take(10) {
        println!(
            "    {diff:>8.1}u ({:>6.2} m)  {t:<6} {edid}  {model}",
            diff / 70.0
        );
    }
    println!("  refs outside terrain coverage: {no_terrain}, NIFs not found/unparsed: {no_nif}");

    // ---- Part D: door vs terrain ----
    println!("\n=== D. DOOR vs TERRAIN ===");
    // XTEL arrival point for the inn's front door (from
    // riverwood_probe): (23259.8, -44476.2, -8.3).
    let arrival = (23259.8f32, -44476.2f32, -8.3f32);
    match terrain_at(arrival.0, arrival.1) {
        Some(h) => println!(
            "  inn door arrival ({:.1},{:.1}): arrival Z {:.1}u, terrain {:.1}u, diff {:.1}u ({:.2} m)",
            arrival.0,
            arrival.1,
            arrival.2,
            h,
            arrival.2 - h,
            (arrival.2 - h) / 70.0
        ),
        None => println!("  inn door arrival: no terrain coverage"),
    }
    // The exterior door ref itself (persistent, DOOR base).
    for r in &persist {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(base_id) = form_id_from_field(r, b"NAME") else {
            continue;
        };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        if base.type_str() != "DOOR" {
            continue;
        }
        let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) else {
            continue;
        };
        let Some(terrain) = terrain_at(pos[0], pos[1]) else {
            continue;
        };
        println!(
            "  exterior door ref 0x{:08x} {} pos=({:.1},{:.1},{:.1}) terrain={:.1}u diff={:.1}u ({:.2} m)",
            r.form_id,
            base.edid().unwrap_or_default(),
            pos[0],
            pos[1],
            pos[2],
            terrain,
            pos[2] - terrain,
            (pos[2] - terrain) / 70.0
        );
    }

    // ---- Part B: parent transforms ----
    println!("\n=== B. PARENT TRANSFORMS (shapes under non-identity parent NiNodes) ===");
    let mut nif_paths: HashSet<String> = HashSet::new();
    for r in refs.iter().chain(inn_refs.iter()) {
        nif_paths.insert(r.model_path.clone());
    }
    println!("  unique NIFs audited: {}", nif_paths.len());
    let mut affected_nifs = 0usize;
    let mut affected_shapes = 0usize;
    let mut total_shapes = 0usize;
    let mut worst_parents: Vec<(f32, String, String, f32, f32)> = Vec::new(); // (t, nif, shape, rot, scale)
    for path in &nif_paths {
        let Some(bytes) = archives.read_file(path) else {
            continue;
        };
        let Ok(nif) = NifFile::from_bytes(&bytes) else {
            continue;
        };
        total_shapes += nif.meshes().len();
        let hits = parent_audit(&nif);
        if !hits.is_empty() {
            affected_nifs += 1;
            affected_shapes += hits.len();
            for (shape, t, rot, scale) in hits {
                worst_parents.push((t, path.clone(), shape, rot, scale));
            }
        }
    }
    println!(
        "  NIFs with non-identity parents: {affected_nifs} / {}  |  shapes affected: {affected_shapes} / {total_shapes}",
        nif_paths.len()
    );
    worst_parents.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    println!("  -- 15 largest parent translations --");
    for (t, nif, shape, rot, scale) in worst_parents.iter().take(15) {
        println!(
            "    parent moves shape by {t:>8.1}u ({:>6.2} m) rot {rot:.3} rad scale {scale:.3}  {shape}  in {nif}",
            t / 70.0
        );
    }

    // ---- Part C: alpha ----
    println!("\n=== C. ALPHA (NiAlphaProperty + tree texture transparency) ===");
    let mut alpha_shapes = 0usize;
    let mut alpha_nifs: HashSet<String> = HashSet::new();
    let mut flag_counts: HashMap<u16, usize> = HashMap::new();
    let mut tree_textures: HashSet<String> = HashSet::new();
    for path in &nif_paths {
        let Some(bytes) = archives.read_file(path) else {
            continue;
        };
        let Ok(nif) = NifFile::from_bytes(&bytes) else {
            continue;
        };
        let hits = alpha_audit(&nif);
        if !hits.is_empty() {
            alpha_nifs.insert(path.clone());
            for (_shape, flags, threshold, diffuse) in &hits {
                alpha_shapes += 1;
                *flag_counts.entry(*flags).or_insert(0) += 1;
                if path.to_lowercase().contains("tree") {
                    if let Some(d) = diffuse {
                        tree_textures.insert(d.clone());
                    }
                }
                let _ = threshold;
            }
        }
    }
    println!(
        "  shapes with NiAlphaProperty: {alpha_shapes} in {} NIFs",
        alpha_nifs.len()
    );
    let mut flags: Vec<(&u16, &usize)> = flag_counts.iter().collect();
    flags.sort_by_key(|(f, _)| **f);
    for (f, c) in flags {
        println!("    flags 0x{f:04x}: {c} shapes");
    }
    println!(
        "  tree diffuse textures with alpha properties: {}",
        tree_textures.len()
    );
    for tex in &tree_textures {
        // Decode the DDS, count transparent pixels (alpha < 128).
        let bytes = archives.read_file(tex).or_else(|| {
            // Try the build-machine-stripped path.
            let norm = normalize(tex);
            let stripped = match norm.rfind("/data/") {
                Some(i) => norm[i + 6..].to_string(),
                None => norm,
            };
            archives.read_file(&stripped)
        });
        match bytes.and_then(|b| dovah_dds::DdsImage::from_bytes(&b).ok()) {
            Some(dds) => match dds.decode(0) {
                Ok(img) => {
                    let total = (img.width * img.height) as usize;
                    let mut transparent = 0usize;
                    let mut partial = 0usize;
                    for p in 0..total {
                        let a = img.data[p * 4 + 3];
                        if a < 128 {
                            transparent += 1;
                        } else if a < 255 {
                            partial += 1;
                        }
                    }
                    println!(
                        "    {tex}: {}x{} alpha<128: {:.1}%  partial: {:.1}%",
                        img.width,
                        img.height,
                        transparent as f32 / total as f32 * 100.0,
                        partial as f32 / total as f32 * 100.0
                    );
                }
                Err(e) => println!("    {tex}: decode failed: {e}"),
            },
            None => println!("    {tex}: DDS not found"),
        }
    }
    // ---- Part E: door panel world-space AABB ----
    // A door panel should be ~1 m wide, ~2 m tall, a few cm thick,
    // with its bottom at the ref's Z and its centre near the ref's
    // X/Y. If the rendered panel is small or offset, this shows it.
    println!("\n=== E. DOOR PANELS (world-space AABB, Skyrim units; 70 units = 1 m) ===");
    for door in &door_refs {
        println!(
            "  door 0x{:08x} {} scale={:.3} ref pos=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3})",
            door.form_id,
            door.base_edid,
            door.scale,
            door.position[0],
            door.position[1],
            door.position[2],
            door.rotation[0],
            door.rotation[1],
            door.rotation[2]
        );
        println!("    model: {}", door.model_path);
        let Some(bytes) = archives.read_file(&door.model_path) else {
            println!("    NIF not found");
            continue;
        };
        let Ok(nif) = NifFile::from_bytes(&bytes) else {
            println!("    NIF parse failed");
            continue;
        };
        for mesh in nif.meshes() {
            if mesh.vertices.is_empty() || mesh.indices.is_empty() {
                continue;
            }
            let rot = mesh.transform.rotation;
            let scale = mesh.transform.scale;
            let trans = mesh.transform.translation;
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for v in &mesh.vertices {
                let r = mat_vec(&rot, *v);
                let p = [
                    r[0] * scale + trans[0],
                    r[1] * scale + trans[1],
                    r[2] * scale + trans[2],
                ];
                for i in 0..3 {
                    min[i] = min[i].min(p[i]);
                    max[i] = max[i].max(p[i]);
                }
            }
            let (wmin, wmax) = world_aabb((min, max), door.position, door.rotation, door.scale);
            let size = [
                (wmax[0] - wmin[0]) / 70.0,
                (wmax[1] - wmin[1]) / 70.0,
                (wmax[2] - wmin[2]) / 70.0,
            ];
            println!(
                "    shape '{}': local t=({:.1},{:.1},{:.1}) s={:.3} | world min=({:.1},{:.1},{:.1}) max=({:.1},{:.1},{:.1}) | size {:.2}x{:.2}x{:.2} m | bottom vs ref Z: {:.1}u ({:.2} m)",
                mesh.name,
                trans[0],
                trans[1],
                trans[2],
                scale,
                wmin[0],
                wmin[1],
                wmin[2],
                wmax[0],
                wmax[1],
                wmax[2],
                size[0],
                size[1],
                size[2],
                wmin[2] - door.position[2],
                (wmin[2] - door.position[2]) / 70.0
            );
        }
    }

    // ---- Part F: terrain vs REFR placement Z (M13 ×8 re-proof) ----
    // Every placed ref in the 3×3 (models or not — the research
    // audit's n = 1,298), terrain minus ref Z in metres. Refs
    // sitting on the ground read ≈ 0. This runs against the
    // SHIPPED decode (×8, Part A/D share it), so it re-proves
    // the research finding end to end: expect median ≈ −0.03 m
    // and ~60% within 0.5 m. (The research probe's ×1 numbers
    // were median +0.95 m, 17% — see mod-tools-reference.md §1.)
    println!("\n=== F. TERRAIN vs REFR Z (delta = terrain - ref Z, metres) ===");
    let mut deltas: Vec<f32> = Vec::new();
    let mut named: Vec<(f32, String)> = Vec::new();
    let mut f_no_terrain = 0usize;
    let mut audit_ref = |r: &Record| {
        let kind = r.type_str();
        if kind != "REFR" && kind != "ACHR" {
            return;
        }
        if r.flags & FLAG_INITIALLY_DISABLED != 0 || r.flags & FLAG_IS_MARKER != 0 {
            return;
        }
        let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) else {
            return;
        };
        let base_edid = form_id_from_field(r, b"NAME")
            .and_then(|id| by_form_id.get(&id))
            .map(|b| b.edid().unwrap_or_default())
            .unwrap_or_default();
        if base_edid.starts_with("XMarker") || base_edid.ends_with("Marker") {
            return;
        }
        let Some(terrain) = terrain_at(pos[0], pos[1]) else {
            f_no_terrain += 1;
            return;
        };
        let delta = (terrain - pos[2]) / 70.0;
        deltas.push(delta);
        if base_edid.contains("TreePineForestSnow")
            || base_edid.contains("TreePineShrub01Snow")
            || base_edid.contains("TreeDeadShrubSnow")
            || base_edid.contains("RockShelf01")
            || base_edid.contains("RockM02Snow")
            || base_edid.contains("SignRiverwoodSleepingGiantInn")
            || base_edid.contains("Farmhouse")
            || base_edid.contains("FarmInn01")
        {
            named.push((delta, base_edid));
        }
    };
    for gx in CENTRE.0 - 1..=CENTRE.0 + 1 {
        for gy in CENTRE.1 - 1..=CENTRE.1 + 1 {
            let Some(cell) = tamriel.get(&(gx, gy)) else {
                continue;
            };
            for r in index.children_of(cell.form_id) {
                audit_ref(r);
            }
        }
    }
    for r in &persist {
        audit_ref(r);
    }
    let n = deltas.len();
    let within = deltas.iter().filter(|d| d.abs() <= 0.5).count();
    let mut sorted = deltas.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[n / 2];
    let mean = deltas.iter().sum::<f32>() / n as f32;
    let p10 = sorted[n / 10];
    let p90 = sorted[n * 9 / 10];
    println!(
        "  n={n} median {median:.2} m  mean {mean:.2} m  p10 {p10:.2} m  p90 {p90:.2} m  within 0.5 m: {within}/{n} ({}%)  no terrain: {f_no_terrain}",
        within * 100 / n
    );
    named.sort_by(|a, b| a.1.cmp(&b.1));
    for (delta, edid) in &named {
        println!("    {delta:>7.2} m  {edid}");
    }
    println!("\nDONE");
}
