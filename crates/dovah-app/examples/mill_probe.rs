//! Mill wheel probe (mill-wheel milestone): what does the real
//! data say about the Riverwood lumber mill waterwheel?
//!
//! A. WHEEL REFS: every REFR in the Riverwood 3x3 (+ persistent
//!    cell) whose base model is a waterwheel NIF — ref form ID,
//!    base form ID + EDID, position/rotation, VMAD (scripts) on
//!    the ref and the base.
//! B. WHEEL NIF ANIMATION: for each distinct wheel NIF, dump the
//!    NiControllerSequence blocks (name, controlled targets),
//!    resolve interpolator -> NiTransformData, and print the
//!    rotation keys (axis, times, values). Also dump the node
//!    list with translations + rotation matrices (axle proof).
//!
//! Usage: cargo run -p dovah-app --example mill_probe
//! (run from the workspace root; reads test-data/)

use dovah_bsa::BsaArchive;
use dovah_esm::{Group, GroupType, Plugin, Record};
use dovah_nif::{Block, NifFile};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::Path;

const DATA_DIR: &str = "test-data";
const ESM_PATH: &str = "test-data/Skyrim.esm";
const CENTRE: (i32, i32) = (5, -11);
const PERSISTENT_CELL_FORM_ID: u32 = 0x00000d74;
const TAMRIEL_WRLD: u32 = 0x0000003c;

const FLAG_INITIALLY_DISABLED: u32 = 0x00000800;

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

/// Printable ASCII runs (>= 4 chars) inside a VMAD field — script
/// names show up as readable strings; this is a hint, not a parse.
fn vmad_strings(record: &Record) -> Vec<String> {
    let Some(f) = record.field(b"VMAD") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cur = String::new();
    for &b in &f.data {
        if (0x20..0x7f).contains(&b) {
            cur.push(b as char);
        } else {
            if cur.len() >= 4 {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    if cur.len() >= 4 {
        out.push(cur);
    }
    out
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

fn dump_nif(nif_path: &str, archives: &mut Archives) {
    println!("\n=== NIF: {nif_path} ===");
    let Some(bytes) = archives.read_file(nif_path) else {
        println!("  NOT FOUND in BSAs");
        return;
    };
    let nif = match NifFile::from_bytes(&bytes) {
        Ok(n) => n,
        Err(e) => {
            println!("  PARSE FAILED: {e}");
            return;
        }
    };
    // Block census + sequences.
    let mut seqs = Vec::new();
    let mut unknown = 0usize;
    let mut total = 0usize;
    let mut i = 0i32;
    while let Some(b) = nif.block(i) {
        total += 1;
        match b {
            Block::Unknown { .. } => unknown += 1,
            Block::ControllerSequence(s) => seqs.push((i, s.clone())),
            _ => {}
        }
        i += 1;
    }
    println!(
        "  blocks: {total}, Unknown: {unknown}, sequences: {}",
        seqs.len()
    );
    for spin in nif.spinning_nodes() {
        println!(
            "  SPIN: node='{}' axis={} omega={:.6} rad/s period={:.4} s",
            spin.node_name, spin.axis, spin.angular_velocity, spin.period
        );
    }
    for (idx, s) in &seqs {
        println!(
            "  [{}] NiControllerSequence name='{}' controlled={}",
            idx,
            s.name,
            s.controlled.len()
        );
        for link in &s.controlled {
            print!(
                "      target='{}' interp=[{}]",
                link.target_name, link.interpolator
            );
            match nif.block(link.interpolator) {
                Some(Block::TransformInterpolator(interp)) => {
                    print!(" data=[{}]", interp.data);
                    match nif.block(interp.data) {
                        Some(Block::TransformData(d)) => {
                            println!();
                            println!(
                                "        X keys: {}  Y keys: {}  Z keys: {}",
                                d.x_keys.len(),
                                d.y_keys.len(),
                                d.z_keys.len()
                            );
                            for (axis, keys) in
                                [("X", &d.x_keys), ("Y", &d.y_keys), ("Z", &d.z_keys)]
                            {
                                for k in keys.iter() {
                                    println!("          {axis}: t={:.4} v={:.4} rad ({:.1} deg) fwd={:.3} bwd={:.3}",
                                        k.time, k.value, k.value.to_degrees(), k.fwd_tangent, k.bwd_tangent);
                                }
                            }
                        }
                        Some(other) => println!(
                            " (data block is {:?}, not TransformData)",
                            std::mem::discriminant(other)
                        ),
                        None => println!(" (data block missing)"),
                    }
                }
                Some(_) => println!(" (not a TransformInterpolator)"),
                None => println!(" (missing)"),
            }
        }
    }
    // Node list with transforms (axle proof).
    println!("  --- nodes ---");
    let mut i = 0i32;
    while let Some(b) = nif.block(i) {
        if let Block::Node(n) = b {
            let t = n.av.translation;
            let r = n.av.rotation;
            println!(
                "  [{}] '{}' t=({:.1},{:.1},{:.1}) scale={:.3} children={:?}",
                i, n.av.net.name, t[0], t[1], t[2], n.av.scale, n.children
            );
            println!(
                "        rot=[[{:.3},{:.3},{:.3}],[{:.3},{:.3},{:.3}],[{:.3},{:.3},{:.3}]]",
                r[0][0], r[0][1], r[0][2], r[1][0], r[1][1], r[1][2], r[2][0], r[2][1], r[2][2]
            );
        }
        i += 1;
    }
    // Part C: wheel-mesh geometry in NIF-root space — the thin
    // axis of the vertex AABB is the axle; the pivot is the
    // parent node's origin (parent_transform.translation).
    println!("  --- wheel-mesh geometry (root space) ---");
    for mesh in nif.meshes() {
        if mesh.parent_node != "SawWaterWheel03" && mesh.parent_node != "SawWaterWheel02" {
            continue;
        }
        let pt = mesh.parent_transform.translation;
        let pr = mesh.parent_transform.rotation;
        // Axle candidates in root space: parent Y (convention B)
        // vs parent-rotated local Y (convention A).
        let axle_a = [
            pr[0][0] * 0.0 + pr[0][1] * 1.0 + pr[0][2] * 0.0,
            pr[1][0] * 0.0 + pr[1][1] * 1.0 + pr[1][2] * 0.0,
            pr[2][0] * 0.0 + pr[2][1] * 1.0 + pr[2][2] * 0.0,
        ];
        println!(
            "  mesh '{}' parent='{}' verts={}",
            mesh.name,
            mesh.parent_node,
            mesh.vertices.len()
        );
        println!(
            "    pivot (parent origin) = ({:.1},{:.1},{:.1})",
            pt[0], pt[1], pt[2]
        );
        println!(
            "    axle A (parent-rot Y) = ({:.3},{:.3},{:.3})  axle B (parent Y) = (0,1,0)",
            axle_a[0], axle_a[1], axle_a[2]
        );
        println!(
            "    shape origin (root) = ({:.1},{:.1},{:.1})",
            mesh.transform.translation[0],
            mesh.transform.translation[1],
            mesh.transform.translation[2]
        );
        if mesh.vertices.is_empty() {
            continue;
        }
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for v in &mesh.vertices {
            let w = mesh.transform.apply(*v);
            for a in 0..3 {
                lo[a] = lo[a].min(w[a]);
                hi[a] = hi[a].max(w[a]);
            }
        }
        println!("    root-space AABB lo=({:.1},{:.1},{:.1}) hi=({:.1},{:.1},{:.1}) size=({:.1},{:.1},{:.1})",
            lo[0], lo[1], lo[2], hi[0], hi[1], hi[2], hi[0]-lo[0], hi[1]-lo[1], hi[2]-lo[2]);
        let c = [
            (lo[0] + hi[0]) / 2.0,
            (lo[1] + hi[1]) / 2.0,
            (lo[2] + hi[2]) / 2.0,
        ];
        println!(
            "    AABB centre = ({:.1},{:.1},{:.1}); centre-pivot = ({:.1},{:.1},{:.1})",
            c[0],
            c[1],
            c[2],
            c[0] - pt[0],
            c[1] - pt[1],
            c[2] - pt[2]
        );
        // Cross product of (centre - pivot) with each axle
        // candidate: ~0 means the centre lies on that axle line.
        let d = [c[0] - pt[0], c[1] - pt[1], c[2] - pt[2]];
        let cross = |a: [f32; 3], b: [f32; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let xa = cross(d, axle_a);
        let xb = cross(d, [0.0, 1.0, 0.0]);
        let mag = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        println!(
            "    |d x axleA| = {:.1}  |d x axleB| = {:.1}  (|d| = {:.1})",
            mag(xa),
            mag(xb),
            mag(d)
        );
    }
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

    println!("=== A. WHEEL REFS in Riverwood 3x3 + persistent ===");
    let mut wheel_nifs: HashSet<String> = HashSet::new();
    let mut check = |records: &[&Record], label: &str| {
        for r in records {
            if r.type_str() != "REFR" {
                continue;
            }
            if r.flags & FLAG_INITIALLY_DISABLED != 0 {
                continue;
            }
            let Some(base_id) = form_id_from_field(r, b"NAME") else {
                continue;
            };
            let Some(base) = by_form_id.get(&base_id) else {
                continue;
            };
            let Some(model) = base.field(b"MODL").map(|f| f.as_str()) else {
                continue;
            };
            let lower = model.to_lowercase();
            if !lower.contains("waterwheel") && !lower.contains("mill") {
                continue;
            }
            let (pos, rot) = r
                .field(b"DATA")
                .and_then(|f| parse_placement(&f.data))
                .unwrap_or(([0.0; 3], [0.0; 3]));
            println!(
                "  REFR 0x{:08x} ({}) base 0x{:08x} '{}' [{}]",
                r.form_id,
                label,
                base_id,
                base.edid().unwrap_or_default(),
                base.type_str()
            );
            println!("    model: meshes\\{model}");
            println!(
                "    pos=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3})",
                pos[0], pos[1], pos[2], rot[0], rot[1], rot[2]
            );
            let ref_scripts = vmad_strings(r);
            let base_scripts = vmad_strings(base);
            println!("    VMAD ref: {:?}  base: {:?}", ref_scripts, base_scripts);
            wheel_nifs.insert(format!("meshes/{model}"));
        }
    };
    for gx in CENTRE.0 - 1..=CENTRE.0 + 1 {
        for gy in CENTRE.1 - 1..=CENTRE.1 + 1 {
            if let Some(cell) = grids.get(&(gx, gy)) {
                let label = format!("cell ({gx},{gy}) 0x{:08x}", cell.form_id);
                check(cells.children_of(cell.form_id), &label);
            }
        }
    }
    check(
        cells.children_of(PERSISTENT_CELL_FORM_ID),
        "persistent cell",
    );

    // Part D: every modelled REFR within 35 Bevy metres of the
    // wheel pivot — the sight-line map for the proof renders.
    // (Bevy = Skyrim (x, z, -y) / 70; wheel pivot from the
    // mill_wheel_spawn_real_numbers test.)
    println!("\n=== D. REFS NEAR THE WHEEL (pivot Bevy 285.2,-1.0,632.3) ===");
    let pivot = [285.2f32, -1.0, 632.3];
    let mut near: Vec<(f32, u32, u32, String, String, [f32; 3])> = Vec::new();
    let mut collect = |records: &[&Record]| {
        for r in records {
            if r.type_str() != "REFR" || r.flags & FLAG_INITIALLY_DISABLED != 0 {
                continue;
            }
            let Some(base_id) = form_id_from_field(r, b"NAME") else {
                continue;
            };
            let Some(base) = by_form_id.get(&base_id) else {
                continue;
            };
            if base.field(b"MODL").is_none() {
                continue;
            }
            let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) else {
                continue;
            };
            let b = [pos[0] / 70.0, pos[2] / 70.0, -pos[1] / 70.0];
            let d =
                ((b[0] - pivot[0]).powi(2) + (b[1] - pivot[1]).powi(2) + (b[2] - pivot[2]).powi(2))
                    .sqrt();
            if d > 35.0 {
                continue;
            }
            near.push((
                d,
                r.form_id,
                base_id,
                base.edid().unwrap_or_default().to_string(),
                base.type_str().to_string(),
                b,
            ));
        }
    };
    for gx in CENTRE.0 - 1..=CENTRE.0 + 1 {
        for gy in CENTRE.1 - 1..=CENTRE.1 + 1 {
            if let Some(cell) = grids.get(&(gx, gy)) {
                collect(cells.children_of(cell.form_id));
            }
        }
    }
    collect(cells.children_of(PERSISTENT_CELL_FORM_ID));
    near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    for (d, ref_id, base_id, edid, ty, b) in &near {
        println!(
            "  {d:5.1}m REFR 0x{ref_id:08x} base 0x{base_id:08x} '{edid}' [{ty}] Bevy ({:.1},{:.1},{:.1})",
            b[0], b[1], b[2]
        );
    }

    let mut archives = Archives::open_all(Path::new(DATA_DIR));
    // Part B only dumps the waterwheel NIFs — the broad "mill"
    // filter also caught log piles / windmills / grain mills.
    let mut nifs: Vec<String> = wheel_nifs
        .into_iter()
        .filter(|p| p.to_lowercase().contains("waterwheel"))
        .collect();
    nifs.sort();
    if nifs.is_empty() {
        println!("\n(no wheel refs found — dumping the known wheel NIFs directly)");
        nifs.push("meshes/architecture/farmhouse/lumbermill01waterwheel01.nif".to_string());
        nifs.push("meshes/architecture/farmhouse/lumbermill01waterwheel02.nif".to_string());
    }
    for nif_path in &nifs {
        dump_nif(nif_path, &mut archives);
    }
}
