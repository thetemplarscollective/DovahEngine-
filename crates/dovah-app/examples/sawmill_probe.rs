//! Sawmill probe (sawmill milestone): what does the real data
//! say about the Riverwood lumber mill saw?
//!
//! A. SAWMILL REFS: every REFR in the Riverwood 3x3 (+ persistent
//!    cell) whose base model is a saw / sash / log / log-pile
//!    NIF — ref form ID, base form ID + EDID, position/rotation,
//!    VMAD (scripts) on the ref and the base.
//! B. SAW/SASH NIF ANIMATION: for each distinct NIF, dump the
//!    NiControllerSequence blocks (name, controlled targets),
//!    resolve interpolator -> NiTransformData, and print the
//!    rotation keys AND translation keys. Also `spinning_nodes`
//!    and the node list, plus mesh geometry in NIF-root space
//!    for every animated node (AABB, pivot, axle proof).
//! C. REFS NEAR THE SAWMILL: every modelled REFR within 20 Bevy
//!    metres of the Riverwood sawmill furniture — the sight-line
//!    map for the proof renders.
//! D. THE SCRIPT: `scripts/resourceobjectsawmillscript.pex` —
//!    states, functions, and every call each function makes
//!    (does it play animations on the blade, or only handle the
//!    log-sawing interaction?).
//!
//! Usage: cargo run -p dovah-app --example sawmill_probe
//! (run from the workspace root; reads test-data/)

use dovah_bsa::BsaArchive;
use dovah_esm::{Group, GroupType, Plugin, Record};
use dovah_nif::{Block, NifFile};
use dovah_pex::{Instruction, PexFile, Value};
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
                            // Summarize dense key groups (the
                            // furniture anims carry up to ~1000
                            // keys at 30 fps): counts, time
                            // range, value range, first/last.
                            // Full keys only for short groups.
                            let summarize = |axis: &str, keys: &[dovah_nif::FloatKey]| {
                                if keys.is_empty() {
                                    println!("          {axis}: 0 keys");
                                    return;
                                }
                                let vals: Vec<f32> = keys.iter().map(|k| k.value).collect();
                                let min = vals.iter().cloned().fold(f32::INFINITY, f32::min);
                                let max = vals.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                                println!(
                                    "          {axis}: {} keys t={:.3}..{:.3} range {:.4}..{:.4} rad ({:.1}..{:.1} deg)",
                                    keys.len(),
                                    keys.first().unwrap().time,
                                    keys.last().unwrap().time,
                                    min,
                                    max,
                                    min.to_degrees(),
                                    max.to_degrees()
                                );
                                if keys.len() <= 8 {
                                    for k in keys.iter() {
                                        println!(
                                            "            t={:.4} v={:.4} rad ({:.1} deg) fwd={:.3} bwd={:.3}",
                                            k.time,
                                            k.value,
                                            k.value.to_degrees(),
                                            k.fwd_tangent,
                                            k.bwd_tangent
                                        );
                                    }
                                }
                            };
                            println!(
                                "        X keys: {}  Y keys: {}  Z keys: {}  T keys: {}",
                                d.x_keys.len(),
                                d.y_keys.len(),
                                d.z_keys.len(),
                                d.translation_keys.len()
                            );
                            summarize("X", &d.x_keys);
                            summarize("Y", &d.y_keys);
                            summarize("Z", &d.z_keys);
                            if !d.translation_keys.is_empty() {
                                let ts = &d.translation_keys;
                                let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                                for k in ts.iter() {
                                    for a in 0..3 {
                                        lo[a] = lo[a].min(k.value[a]);
                                        hi[a] = hi[a].max(k.value[a]);
                                    }
                                }
                                println!(
                                    "          T: {} keys t={:.3}..{:.3} x {:.1}..{:.1} y {:.1}..{:.1} z {:.1}..{:.1}",
                                    ts.len(),
                                    ts.first().unwrap().time,
                                    ts.last().unwrap().time,
                                    lo[0],
                                    hi[0],
                                    lo[1],
                                    hi[1],
                                    lo[2],
                                    hi[2]
                                );
                                if ts.len() <= 8 {
                                    for k in ts.iter() {
                                        println!(
                                            "            t={:.4} v=({:.1},{:.1},{:.1})",
                                            k.time, k.value[0], k.value[1], k.value[2]
                                        );
                                    }
                                }
                            }
                        }
                        Some(other) => {
                            println!(" (data block is {}, not TransformData)", other.type_name())
                        }
                        None => println!(" (data block missing)"),
                    }
                }
                Some(_) => println!(" (not a TransformInterpolator)"),
                None => println!(" (missing)"),
            }
        }
    }
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
    // Geometry in NIF-root space for every animated node: AABB,
    // pivot (parent origin), axle candidates.
    let animated: HashSet<String> = seqs
        .iter()
        .flat_map(|(_, s)| s.controlled.iter().map(|l| l.target_name.clone()))
        .collect();
    println!("  --- animated-node mesh geometry (root space) ---");
    for mesh in nif.meshes() {
        if !animated.contains(&mesh.parent_node) {
            continue;
        }
        let pt = mesh.parent_transform.translation;
        let pr = mesh.parent_transform.rotation;
        let axle_y = [pr[0][1], pr[1][1], pr[2][1]];
        let axle_x = [pr[0][0], pr[1][0], pr[2][0]];
        let axle_z = [pr[0][2], pr[1][2], pr[2][2]];
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
            "    axleX=({:.3},{:.3},{:.3}) axleY=({:.3},{:.3},{:.3}) axleZ=({:.3},{:.3},{:.3})",
            axle_x[0],
            axle_x[1],
            axle_x[2],
            axle_y[0],
            axle_y[1],
            axle_y[2],
            axle_z[0],
            axle_z[1],
            axle_z[2]
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
        println!(
            "    root-space AABB lo=({:.1},{:.1},{:.1}) hi=({:.1},{:.1},{:.1}) size=({:.1},{:.1},{:.1})",
            lo[0],
            lo[1],
            lo[2],
            hi[0],
            hi[1],
            hi[2],
            hi[0] - lo[0],
            hi[1] - lo[1],
            hi[2] - lo[2]
        );
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
        let d = [c[0] - pt[0], c[1] - pt[1], c[2] - pt[2]];
        let cross = |a: [f32; 3], b: [f32; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let mag = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        println!(
            "    |d x axleX| = {:.1}  |d x axleY| = {:.1}  |d x axleZ| = {:.1}  (|d| = {:.1})",
            mag(cross(d, axle_x)),
            mag(cross(d, axle_y)),
            mag(cross(d, axle_z)),
            mag(d)
        );
    }
}

/// One-line summary of a call instruction: `callmethod
/// MethodName on ObjectName` / `callstatic Type.Method` /
/// `callparent MethodName`. Method-first per dovah-papyrus
/// semantics (proven over 112,513 callmethods).
fn call_summary(ins: &Instruction) -> Option<String> {
    let name_of = |v: &Value| match v {
        Value::Identifier(s) | Value::Str(s) => Some(s.clone()),
        _ => None,
    };
    match ins.op {
        0x17 => {
            // callmethod (method, object, dest, args...)
            let m = ins.args.first().and_then(name_of)?;
            let o = ins.args.get(1).and_then(name_of).unwrap_or_default();
            Some(format!("callmethod {m} on {o}"))
        }
        0x18 => {
            // callparent (method, dest, args...)
            let m = ins.args.first().and_then(name_of)?;
            Some(format!("callparent {m}"))
        }
        0x19 => {
            // callstatic (type, method, dest, args...)
            let t = ins.args.first().and_then(name_of).unwrap_or_default();
            let m = ins.args.get(1).and_then(name_of)?;
            Some(format!("callstatic {t}.{m}"))
        }
        _ => None,
    }
}

fn dump_pex(pex_path: &str, archives: &mut Archives) {
    println!("\n=== PEX: {pex_path} ===");
    let Some(bytes) = archives.read_file(pex_path) else {
        println!("  NOT FOUND in BSAs");
        return;
    };
    let pex = match PexFile::from_bytes(&bytes) {
        Ok(p) => p,
        Err(e) => {
            println!("  PARSE FAILED: {e}");
            return;
        }
    };
    for obj in &pex.objects {
        println!("  object '{}' extends '{}'", obj.name, obj.parent);
        for var in &obj.variables {
            println!(
                "    var {} : {} = {:?}",
                var.name, var.type_name, var.default
            );
        }
        for prop in &obj.properties {
            println!(
                "    property {} : {} (read={} write={} auto={})",
                prop.name,
                prop.type_name,
                prop.can_read(),
                prop.can_write(),
                prop.is_auto()
            );
        }
        for state in &obj.states {
            println!(
                "    state '{}' ({} functions)",
                state.name,
                state.functions.len()
            );
            for nf in &state.functions {
                let f = &nf.function;
                println!(
                    "      fn {}({}) -> {} [{} instructions{}]",
                    nf.name,
                    f.params
                        .iter()
                        .map(|p| format!("{}: {}", p.name, p.type_name))
                        .collect::<Vec<_>>()
                        .join(", "),
                    f.return_type,
                    f.instructions.len(),
                    if f.is_native() { ", native" } else { "" }
                );
                if !f.docstring.is_empty() {
                    println!("        doc: {}", f.docstring);
                }
                let mut calls: Vec<String> = Vec::new();
                for ins in &f.instructions {
                    if let Some(s) = call_summary(ins) {
                        calls.push(s);
                    }
                }
                for c in &calls {
                    println!("        {c}");
                }
                // Full string arguments of the animation calls —
                // which sequence names the script plays / listens
                // for.
                for ins in &f.instructions {
                    if !matches!(ins.op, 0x17 | 0x18 | 0x19) {
                        continue;
                    }
                    let strs: Vec<&str> = ins
                        .args
                        .iter()
                        .filter_map(|a| match a {
                            Value::Str(s) | Value::Identifier(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .collect();
                    if !strs.is_empty() {
                        println!("        args: {:?}", strs);
                    }
                }
            }
        }
    }
    // String-table entries that look like animation / event names.
    println!("  --- string table (animation-ish) ---");
    for s in &pex.strings {
        let l = s.to_lowercase();
        if l.contains("dust")
            || l.contains("cut")
            || l.contains("saw")
            || l.contains("load")
            || l.contains("pile")
            || l.contains("activate")
            || l.contains("jam")
            || l.contains("sabotage")
            || l.contains("work")
        {
            println!("    {s:?}");
        }
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

    println!("=== A. SAWMILL REFS in Riverwood 3x3 + persistent ===");
    let mut saw_nifs: HashSet<String> = HashSet::new();
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
            if !lower.contains("lumbermillsaw")
                && !lower.contains("lumbermillsash")
                && !lower.contains("lumbermilllog")
                && !lower.contains("milllogpile")
            {
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
            saw_nifs.insert(format!("meshes/{model}"));
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

    // Part C: every modelled REFR within 20 Bevy metres of the
    // Riverwood sawmill furniture (Bevy 275.4, 1.3, 633.0) —
    // the sight-line map for the proof renders.
    println!("\n=== C. REFS NEAR THE SAWMILL (Bevy 275.4,1.3,633.0) ===");
    let pivot = [275.4f32, 1.3, 633.0];
    let mut near: Vec<(f32, u32, u32, String, String, [f32; 3], String)> = Vec::new();
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
            let Some(model) = base.field(b"MODL").map(|f| f.as_str().to_string()) else {
                continue;
            };
            let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) else {
                continue;
            };
            let b = [pos[0] / 70.0, pos[2] / 70.0, -pos[1] / 70.0];
            let d =
                ((b[0] - pivot[0]).powi(2) + (b[1] - pivot[1]).powi(2) + (b[2] - pivot[2]).powi(2))
                    .sqrt();
            if d > 20.0 {
                continue;
            }
            near.push((
                d,
                r.form_id,
                base_id,
                base.edid().unwrap_or_default().to_string(),
                base.type_str().to_string(),
                b,
                model,
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
    for (d, ref_id, base_id, edid, ty, b, model) in &near {
        println!(
            "  {d:5.1}m REFR 0x{ref_id:08x} base 0x{base_id:08x} '{edid}' [{ty}] Bevy ({:.1},{:.1},{:.1}) {model}",
            b[0], b[1], b[2]
        );
    }

    let mut archives = Archives::open_all(Path::new(DATA_DIR));
    let mut nifs: Vec<String> = saw_nifs.into_iter().collect();
    // Always dump the standalone saw/sash/log NIFs too — they
    // carry the blade/sash models even if no Riverwood ref uses
    // them (the Riverwood furniture NIF embeds its own rig).
    for extra in [
        "meshes/clutter/lumbermill/lumbermillsaw01/lumbermillsaw01.nif",
        "meshes/clutter/lumbermill/lumbermillsash01/lumbermillsash01.nif",
        "meshes/clutter/lumbermill/lumbermilllog01/lumbermilllog01.nif",
    ] {
        if !nifs.iter().any(|n| n.to_lowercase() == extra) {
            nifs.push(extra.to_string());
        }
    }
    nifs.sort();
    for nif_path in &nifs {
        dump_nif(nif_path, &mut archives);
    }

    // Part E: every base in Skyrim.esm whose model is a
    // saw/sash/log NIF, and how many refs use it ANYWHERE in
    // Tamriel (not just Riverwood).
    println!("\n=== E. SAW/SASH/LOG BASES ANYWHERE IN SKYRIM.ESM ===");
    let mut saw_bases: HashMap<u32, (String, String, String)> = HashMap::new();
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        for r in g.records() {
            if r.type_str() == "REFR" {
                continue;
            }
            if let Some(model) = r.field(b"MODL").map(|f| f.as_str().to_string()) {
                let lower = model.to_lowercase();
                if lower.contains("lumbermillsaw")
                    || lower.contains("lumbermillsash")
                    || lower.contains("lumbermilllog")
                {
                    saw_bases.insert(
                        r.form_id,
                        (
                            r.edid().unwrap_or_default().to_string(),
                            r.type_str().to_string(),
                            model,
                        ),
                    );
                }
            }
        }
        stack.extend(g.subgroups());
    }
    let mut ref_counts: HashMap<u32, Vec<(u32, [f32; 3])>> = HashMap::new();
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        for r in g.records() {
            if r.type_str() != "REFR" {
                continue;
            }
            if let Some(base_id) = form_id_from_field(r, b"NAME") {
                if saw_bases.contains_key(&base_id) {
                    let pos = r
                        .field(b"DATA")
                        .and_then(|f| parse_placement(&f.data))
                        .map(|(p, _)| p)
                        .unwrap_or([0.0; 3]);
                    ref_counts
                        .entry(base_id)
                        .or_default()
                        .push((r.form_id, pos));
                }
            }
        }
        stack.extend(g.subgroups());
    }
    if saw_bases.is_empty() {
        println!("  (no bases use a saw/sash/log model)");
    }
    for (base_id, (edid, ty, model)) in &saw_bases {
        let refs = ref_counts.get(base_id).map(|v| v.as_slice()).unwrap_or(&[]);
        println!(
            "  base 0x{base_id:08x} '{edid}' [{ty}] {model} — {} refs",
            refs.len()
        );
        for (ref_id, pos) in refs {
            let b = [pos[0] / 70.0, pos[2] / 70.0, -pos[1] / 70.0];
            println!(
                "    REFR 0x{ref_id:08x} Bevy ({:.1},{:.1},{:.1})",
                b[0], b[1], b[2]
            );
        }
    }

    // Part G: every FURN (animated-furniture record type) NIF
    // used by refs in the Riverwood exterior cells, plus any NIF
    // path mentioning mill/saw/grind/wheel — the complete
    // answer to "what would generic direct-controller wiring
    // animate out here besides the sawmill blade?"
    println!("\n=== G. FURN / MILL-ISH NIFS IN THE EXTERIOR CELLS ===");
    {
        let mut furn_refs: Vec<(u32, u32, String, String)> = Vec::new();
        let mut millish: HashSet<String> = HashSet::new();
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
                let Some(model) = base.field(b"MODL").map(|f| f.as_str()) else {
                    continue;
                };
                let lower = model.to_lowercase();
                if base.type_str() == "FURN" {
                    furn_refs.push((
                        r.form_id,
                        base_id,
                        base.edid().unwrap_or_default().to_string(),
                        model.to_string(),
                    ));
                }
                if lower.contains("mill")
                    || lower.contains("saw")
                    || lower.contains("grind")
                    || lower.contains("wheel")
                {
                    millish.insert(model.to_string());
                }
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
        println!("  FURN refs in cells: {}", furn_refs.len());
        for (ref_id, base_id, edid, model) in &furn_refs {
            println!("    ref 0x{ref_id:08x} base 0x{base_id:08x} '{edid}' {model}");
        }
        println!("  mill/saw/grind/wheel NIFs:");
        let mut millish: Vec<&String> = millish.iter().collect();
        millish.sort();
        for m in millish {
            println!("    {m}");
        }
    }

    // Part F: the SawGear blade in Bevy world space — where to
    // point the proof camera. Ref 0x00071dfc (Riverwood sawmill
    // furniture): pos (19277.3,-44313.2,90.9), yaw 0.790.
    // SawGear node: NIF offset (-24.3,648.9,-11.9), rest rotation
    // Rz(90°), so its local Y (the spin axis) maps to NIF-root
    // -X. Computed with the engine's own coords fns.
    println!("\n=== F. SAWGEAR BLADE IN WORLD SPACE ===");
    {
        use bevy::prelude::*;
        use dovah_app::coords;
        let ref_transform = Transform {
            translation: coords::to_bevy_position([19277.3, -44313.2, 90.9]),
            rotation: coords::to_bevy_rotation([0.0, 0.0, 0.790]),
            scale: Vec3::ONE,
        };
        let pivot_nif = [-24.3f32, 648.9, -11.9];
        let axle_nif = [-1.0f32, 0.0, 0.0]; // local Y through Rz(90°)
        let pivot_world = ref_transform.transform_point(coords::to_bevy_position(pivot_nif));
        let axle_world =
            (ref_transform.rotation * coords::to_bevy_direction(axle_nif)).normalize_or_zero();
        println!(
            "  blade pivot (world Bevy): ({:.2},{:.2},{:.2})",
            pivot_world.x, pivot_world.y, pivot_world.z
        );
        println!(
            "  blade axle (world Bevy): ({:.3},{:.3},{:.3})",
            axle_world.x, axle_world.y, axle_world.z
        );
        // The disc is ~140 units (2.0 m) across; park the camera
        // 4 m off along the axle and 1.5 m up, looking at the pivot.
        let cam = pivot_world + axle_world * 4.0 + Vec3::new(0.0, 1.5, 0.0);
        println!(
            "  suggested camera: --camera {:.1},{:.1},{:.1}:{:.1},{:.1},{:.1}",
            cam.x, cam.y, cam.z, pivot_world.x, pivot_world.y, pivot_world.z
        );
        // And the same from the other side of the disc.
        let cam2 = pivot_world - axle_world * 4.0 + Vec3::new(0.0, 1.5, 0.0);
        println!(
            "  other side:        --camera {:.1},{:.1},{:.1}:{:.1},{:.1},{:.1}",
            cam2.x, cam2.y, cam2.z, pivot_world.x, pivot_world.y, pivot_world.z
        );
    }

    dump_pex("scripts/resourceobjectsawmillscript.pex", &mut archives);
}
