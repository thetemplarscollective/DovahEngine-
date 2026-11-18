//! Forge probe (forge milestone): what does the real data
//! say about Alvor's forge in Riverwood?
//!
//! A. FORGE REFS: every REFR in the Riverwood 3x3 (+ persistent
//!    cell) whose base model mentions forge / anvil / bellows /
//!    sharpening / workbench / smith — ref form ID, base form ID
//!    + EDID, position/rotation, VMAD (scripts) on ref and base.
//! B. FORGE NIF ANIMATION + FIRE: for each distinct NIF — block
//!    census (incl. particle-system Unknown types), sequences,
//!    `spinning_nodes`, `direct_controllers` (with key
//!    summaries), emissive shader properties, node list, and
//!    animated-node mesh geometry (AABB, pivot, axle proof).
//! C. LIGHTS NEAR THE FORGE: LIGH refs within 15 Bevy m of the
//!    forge pivot (radius / color / flags / fade).
//! D. REFS NEAR THE FORGE: every modelled REFR within 20 Bevy
//!    m of the pivot — the sight-line map for the proof renders.
//! E. GRINDSTONE WHEEL IN WORLD SPACE: pivot / axle + suggested
//!    proof cameras.
//!
//! Usage: cargo run -p dovah-app --example forge_probe
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

/// Summarize one rotation key group: counts, time range, value
/// range; full keys only for short groups (furniture anims carry
/// up to ~1000 keys at 30 fps).
fn summarize(axis: &str, keys: &[dovah_nif::FloatKey]) {
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
    // Block census: particle systems + anything else the parser
    // does not know parse as Unknown — their type names are the
    // fire evidence.
    let mut total = 0usize;
    let mut unknown_types: HashMap<String, usize> = HashMap::new();
    let mut seqs = Vec::new();
    let mut i = 0i32;
    while let Some(b) = nif.block(i) {
        total += 1;
        match b {
            Block::Unknown { type_name } => {
                *unknown_types.entry(type_name.clone()).or_default() += 1;
            }
            Block::ControllerSequence(s) => seqs.push((i, s.clone())),
            _ => {}
        }
        i += 1;
    }
    println!("  blocks: {total}, sequences: {}", seqs.len());
    if !unknown_types.is_empty() {
        let mut types: Vec<(&String, &usize)> = unknown_types.iter().collect();
        types.sort();
        for (t, n) in types {
            println!("    Unknown {t}: {n}");
        }
    }
    for spin in nif.spinning_nodes() {
        println!(
            "  SPIN: node='{}' axis={} omega={:.6} rad/s period={:.4} s",
            spin.node_name, spin.axis, spin.angular_velocity, spin.period
        );
    }
    for ctl in nif.direct_controllers() {
        println!(
            "  DIRECT: node='{}' flags=0x{:04x} freq={} phase={} t={:.3}..{:.3}",
            ctl.node_name, ctl.flags, ctl.frequency, ctl.phase, ctl.start_time, ctl.stop_time
        );
        summarize("X", &ctl.data.x_keys);
        summarize("Y", &ctl.data.y_keys);
        summarize("Z", &ctl.data.z_keys);
        if !ctl.data.translation_keys.is_empty() {
            let ts = &ctl.data.translation_keys;
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
        }
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
                                "        X keys: {}  Y keys: {}  Z keys: {}  T keys: {}",
                                d.x_keys.len(),
                                d.y_keys.len(),
                                d.z_keys.len(),
                                d.translation_keys.len()
                            );
                            summarize("X", &d.x_keys);
                            summarize("Y", &d.y_keys);
                            summarize("Z", &d.z_keys);
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
    // Emissive shader properties: the coals' glow, if any.
    let mut i = 0i32;
    let mut emissive_found = false;
    while let Some(b) = nif.block(i) {
        if let Block::LightingShaderProperty(p) = b {
            let c = p.emissive_color;
            if p.emissive_multiple > 0.0 && (c[0] > 0.0 || c[1] > 0.0 || c[2] > 0.0) {
                println!(
                    "  EMISSIVE [{}] '{}' color=({:.3},{:.3},{:.3}) x{:.2}",
                    i, p.net.name, c[0], c[1], c[2], p.emissive_multiple
                );
                emissive_found = true;
            }
        }
        if let Block::EffectShaderProperty(p) = b {
            let c = p.emissive;
            if c[0] > 0.0 || c[1] > 0.0 || c[2] > 0.0 {
                println!(
                    "  EFFECT-EMISSIVE [{}] '{}' color=({:.3},{:.3},{:.3},{:.3})",
                    i, p.net.name, c[0], c[1], c[2], c[3]
                );
                emissive_found = true;
            }
        }
        i += 1;
    }
    if !emissive_found {
        println!("  (no emissive shader properties)");
    }
    println!("  --- nodes ---");
    let mut i = 0i32;
    while let Some(b) = nif.block(i) {
        if let Block::Node(n) = b {
            let t = n.av.translation;
            println!(
                "  [{}] '{}' t=({:.1},{:.1},{:.1}) scale={:.3} children={:?}",
                i, n.av.net.name, t[0], t[1], t[2], n.av.scale, n.children
            );
        }
        i += 1;
    }
    // Geometry in NIF-root space for every animated node: AABB,
    // pivot (parent origin), axle candidates. Vertices are
    // shape-local — apply the shape's composed transform first
    // (the raw AABB misled the first pass: the grindstone
    // wheel's shape-local AABB sits 134 units from its pivot).
    // The AABB centre's distance to the axle line through the
    // pivot is the spin proof (a spinning disc is centred on
    // its axle; an orbiting mesh is not).
    let mut animated: HashSet<String> = seqs
        .iter()
        .flat_map(|(_, s)| s.controlled.iter().map(|l| l.target_name.clone()))
        .collect();
    for spin in nif.spinning_nodes() {
        animated.insert(spin.node_name.clone());
    }
    for ctl in nif.direct_controllers() {
        animated.insert(ctl.node_name.clone());
    }
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
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for v in &mesh.vertices {
            let w = mesh.transform.apply(*v);
            for a in 0..3 {
                lo[a] = lo[a].min(w[a]);
                hi[a] = hi[a].max(w[a]);
            }
        }
        let centre = [
            (lo[0] + hi[0]) / 2.0,
            (lo[1] + hi[1]) / 2.0,
            (lo[2] + hi[2]) / 2.0,
        ];
        println!(
            "  mesh '{}' parent='{}' verts={} AABB x {:.1}..{:.1} y {:.1}..{:.1} z {:.1}..{:.1}",
            mesh.name,
            mesh.parent_node,
            mesh.vertices.len(),
            lo[0],
            hi[0],
            lo[1],
            hi[1],
            lo[2],
            hi[2]
        );
        println!(
            "    pivot (parent origin) = ({:.1},{:.1},{:.1}) centre=({:.1},{:.1},{:.1})",
            pt[0], pt[1], pt[2], centre[0], centre[1], centre[2]
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
        // Distance from the AABB centre to each axle line
        // through the pivot (|d x axle| for unit axle).
        for (name, ax) in [("X", axle_x), ("Y", axle_y), ("Z", axle_z)] {
            let d = [centre[0] - pt[0], centre[1] - pt[1], centre[2] - pt[2]];
            let cx = d[1] * ax[2] - d[2] * ax[1];
            let cy = d[2] * ax[0] - d[0] * ax[2];
            let cz = d[0] * ax[1] - d[1] * ax[0];
            let dist = (cx * cx + cy * cy + cz * cz).sqrt();
            println!("    |centre - pivot| x axle{name} = {dist:.1}");
        }
    }
}

fn is_forge_model(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.contains("forge")
        || lower.contains("anvil")
        || lower.contains("bellows")
        || lower.contains("sharpening")
        || lower.contains("blacksmith")
        || lower.contains("smithing")
}

fn dump_fire_textures(archives: &mut Archives) {
    // Part F: why the forge fire renders as white planes —
    // dump the resolved textures (diffuse/gradient) + blend
    // mode + emissive for every effect-shader (unlit) mesh
    // and every emissive lighting-shader mesh in the marker
    // NIF, as the engine's own meshes() resolves them.
    println!("\n=== F. FORGE FIRE MESH TEXTURES (marker NIF) ===");
    let path = "meshes/furniture/blacksmithforgemarker.nif";
    let Some(bytes) = archives.read_file(path) else {
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
    for mesh in nif.meshes() {
        if mesh.unlit || mesh.emissive != [0.0, 0.0, 0.0] {
            println!(
                "  mesh '{}' parent='{}' unlit={} diffuse={:?} gradient={:?} blend={:?} emissive=[{:.2},{:.2},{:.2}]",
                mesh.name,
                mesh.parent_node,
                mesh.unlit,
                mesh.textures.diffuse,
                mesh.textures.gradient,
                mesh.alpha_blend,
                mesh.emissive[0],
                mesh.emissive[1],
                mesh.emissive[2]
            );
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

    println!("=== A. FORGE REFS in Riverwood 3x3 + persistent ===");
    let mut forge_nifs: HashSet<String> = HashSet::new();
    // Pivot for Parts C/D: the forge marker ref (Alvor's
    // forge) if found, else the forge grindstone, else the
    // approx. (The first pass used grindstone 0x0006f947 —
    // the MILL grindstone 33 m north — and missed the forge.)
    let mut pivot: [f32; 3] = [292.3, -1.2, 646.1];
    let mut pivot_from = "fallback (Alvor's forge approx)";
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
            if !is_forge_model(&model) {
                continue;
            }
            let (pos, rot) = r
                .field(b"DATA")
                .and_then(|f| parse_placement(&f.data))
                .unwrap_or(([0.0; 3], [0.0; 3]));
            let b = [pos[0] / 70.0, pos[2] / 70.0, -pos[1] / 70.0];
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
                "    pos=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3}) Bevy=({:.1},{:.1},{:.1})",
                pos[0], pos[1], pos[2], rot[0], rot[1], rot[2], b[0], b[1], b[2]
            );
            let ref_scripts = vmad_strings(r);
            let base_scripts = vmad_strings(base);
            println!("    VMAD ref: {:?}  base: {:?}", ref_scripts, base_scripts);
            forge_nifs.insert(format!("meshes/{model}"));
            if r.form_id == 0x000d6987 {
                pivot = b;
                pivot_from = "forge marker ref 0x000d6987";
            }
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
    println!(
        "  pivot for C/D: ({:.1},{:.1},{:.1}) [{pivot_from}]",
        pivot[0], pivot[1], pivot[2]
    );

    // Part A2: the other four plugins (Update/Dawnguard/
    // HearthFires/Dragonborn) can add or move refs — an anvil
    // the base game lacks might live there. Scan every REFR
    // in each plugin whose base (resolved in Skyrim.esm or
    // the plugin itself) has a forge-model path, and print
    // those within 30 Bevy m of the forge pivot.
    println!("\n=== A2. FORGE REFS IN THE OTHER PLUGINS (near Riverwood) ===");
    for extra in [
        "test-data/Update.esm",
        "test-data/Dawnguard.esm",
        "test-data/HearthFires.esm",
        "test-data/Dragonborn.esm",
    ] {
        let Ok(xplugin) = Plugin::open(Path::new(extra)) else {
            println!("  {extra}: could not open");
            continue;
        };
        let mut x_by_form_id: HashMap<u32, &Record> = HashMap::new();
        let mut xstack: Vec<&Group> = xplugin.groups.iter().collect();
        while let Some(g) = xstack.pop() {
            for r in g.records() {
                x_by_form_id.insert(r.form_id, r);
            }
            xstack.extend(g.subgroups());
        }
        let mut found = 0usize;
        let mut xstack2: Vec<&Group> = xplugin.groups.iter().collect();
        while let Some(g) = xstack2.pop() {
            for r in g.records() {
                if r.type_str() != "REFR" || r.flags & FLAG_INITIALLY_DISABLED != 0 {
                    continue;
                }
                let Some(base_id) = form_id_from_field(r, b"NAME") else {
                    continue;
                };
                let base = x_by_form_id
                    .get(&base_id)
                    .copied()
                    .or_else(|| by_form_id.get(&base_id).copied());
                let Some(base) = base else { continue };
                let Some(model) = base.field(b"MODL").map(|f| f.as_str()) else {
                    continue;
                };
                if !is_forge_model(&model) {
                    continue;
                }
                let Some((pos, rot)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data))
                else {
                    continue;
                };
                let b = [pos[0] / 70.0, pos[2] / 70.0, -pos[1] / 70.0];
                let d = ((b[0] - pivot[0]).powi(2)
                    + (b[1] - pivot[1]).powi(2)
                    + (b[2] - pivot[2]).powi(2))
                .sqrt();
                if d > 30.0 {
                    continue;
                }
                found += 1;
                println!(
                    "  {extra} REFR 0x{:08x} base 0x{:08x} '{}' [{}] Bevy=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3}) model={}",
                    r.form_id,
                    base_id,
                    base.edid().unwrap_or_default(),
                    base.type_str(),
                    b[0],
                    b[1],
                    b[2],
                    rot[0],
                    rot[1],
                    rot[2],
                    model
                );
            }
            xstack2.extend(g.subgroups());
        }
        if found == 0 {
            println!("  {extra}: no forge-model refs within 30 m of the forge");
        }
    }

    // Part C: LIGH refs near the forge (the fire light has no
    // MODL, so Part A misses it).
    println!("\n=== C. LIGHT REFS NEAR THE FORGE (15 m) ===");
    let mut lights: Vec<(f32, u32, u32, String, [f32; 3], u32, u32, u32, f32)> = Vec::new();
    let mut collect_lights = |records: &[&Record]| {
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
            if base.type_str() != "LIGH" {
                continue;
            }
            let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) else {
                continue;
            };
            let b = [pos[0] / 70.0, pos[2] / 70.0, -pos[1] / 70.0];
            let d =
                ((b[0] - pivot[0]).powi(2) + (b[1] - pivot[1]).powi(2) + (b[2] - pivot[2]).powi(2))
                    .sqrt();
            if d > 15.0 {
                continue;
            }
            // LIGH DATA 48B: radius u32 @4, color 0x00BBGGRR @8,
            // flags @12; FNAM = Fade f32.
            let (radius, color, flags) = match base.field(b"DATA") {
                Some(f) if f.data.len() >= 16 => (
                    u32::from_le_bytes(f.data[4..8].try_into().unwrap()),
                    u32::from_le_bytes(f.data[8..12].try_into().unwrap()),
                    u32::from_le_bytes(f.data[12..16].try_into().unwrap()),
                ),
                _ => (0, 0, 0),
            };
            let fade = base
                .field(b"FNAM")
                .filter(|f| f.data.len() == 4)
                .map(|f| f32::from_le_bytes(f.data[0..4].try_into().unwrap()))
                .unwrap_or(0.0);
            lights.push((
                d,
                r.form_id,
                base_id,
                base.edid().unwrap_or_default().to_string(),
                b,
                radius,
                color,
                flags,
                fade,
            ));
        }
    };
    for gx in CENTRE.0 - 1..=CENTRE.0 + 1 {
        for gy in CENTRE.1 - 1..=CENTRE.1 + 1 {
            if let Some(cell) = grids.get(&(gx, gy)) {
                collect_lights(cells.children_of(cell.form_id));
            }
        }
    }
    collect_lights(cells.children_of(PERSISTENT_CELL_FORM_ID));
    lights.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    if lights.is_empty() {
        println!("  (no LIGH refs within 15 m of the forge)");
    }
    for (d, ref_id, base_id, edid, b, radius, color, flags, fade) in &lights {
        println!(
            "  {d:5.1}m REFR 0x{ref_id:08x} base 0x{base_id:08x} '{edid}' Bevy ({:.1},{:.1},{:.1}) radius={radius}u color=0x{color:06x} flags=0x{flags:04x} fade={fade}",
            b[0], b[1], b[2]
        );
    }

    // Part D: every modelled REFR within 20 Bevy m of the pivot
    // — the sight-line map for the proof renders.
    println!("\n=== D. REFS NEAR THE FORGE (20 m) ===");
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
    let mut nifs: Vec<String> = forge_nifs.into_iter().collect();
    nifs.sort();
    for nif_path in &nifs {
        dump_nif(nif_path, &mut archives);
    }

    // Part E: the grindstone Wheels in world space — where to
    // point the proof cameras. Wheel node pivot NIF
    // (10.8, 97.1, 34.2), spin axis local X = NIF-root +X
    // (identity parent rotation, probe Part B). Ref
    // transforms from Part A. Computed with the engine's own
    // coords fns (sawmill Part F pattern).
    println!("\n=== E. GRINDSTONE WHEELS IN WORLD SPACE ===");
    {
        use bevy::prelude::*;
        use dovah_app::coords;
        for (label, pos, rot) in [
            (
                "forge grindstone 0x0001c785",
                [20375.4, -45066.6, -80.0],
                [0.0, 0.0, 1.551],
            ),
            (
                "mill grindstone 0x0006f947",
                [20455.2, -42892.8, -111.1],
                [0.083, 0.000, 5.096],
            ),
        ] {
            let ref_transform = Transform {
                translation: coords::to_bevy_position(pos),
                rotation: coords::to_bevy_rotation(rot),
                scale: Vec3::ONE,
            };
            let pivot_nif = [10.8f32, 97.1, 34.2];
            let axle_nif = [1.0f32, 0.0, 0.0]; // local X, identity parents
            let pivot_world = ref_transform.transform_point(coords::to_bevy_position(pivot_nif));
            let axle_world =
                (ref_transform.rotation * coords::to_bevy_direction(axle_nif)).normalize_or_zero();
            println!("  {label}:");
            println!(
                "    wheel pivot (world Bevy): ({:.2},{:.2},{:.2})",
                pivot_world.x, pivot_world.y, pivot_world.z
            );
            println!(
                "    wheel axle (world Bevy): ({:.3},{:.3},{:.3})",
                axle_world.x, axle_world.y, axle_world.z
            );
            // The wheel is ~0.44 m across; park the camera
            // 2.5 m off along the axle and 0.8 m up.
            let cam = pivot_world + axle_world * 2.5 + Vec3::new(0.0, 0.8, 0.0);
            println!(
                "    camera +axle: --camera {:.1},{:.1},{:.1}:{:.1},{:.1},{:.1}",
                cam.x, cam.y, cam.z, pivot_world.x, pivot_world.y, pivot_world.z
            );
            let cam2 = pivot_world - axle_world * 2.5 + Vec3::new(0.0, 0.8, 0.0);
            println!(
                "    camera -axle: --camera {:.1},{:.1},{:.1}:{:.1},{:.1},{:.1}",
                cam2.x, cam2.y, cam2.z, pivot_world.x, pivot_world.y, pivot_world.z
            );
        }
        // Forge overview: the marker ref at Bevy (292.3,-1.2,646.1);
        // the forge faces the street (north-west). A camera 6 m
        // out looking back at the forge mouth.
        println!("  forge overview: --camera 286.0,2.0,640.0:292.3,-0.5,646.1");
    }

    dump_fire_textures(&mut archives);
}
