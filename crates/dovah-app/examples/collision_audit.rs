//! Collision audit (per-object collision milestone): which Havok
//! (`bhk*`) block types do Riverwood's NIFs actually carry, and
//! which refs have authored collision at all?
//!
//! A. bhk BLOCK CENSUS: over every unique NIF referenced by the
//!    exterior 3x3 + inn refs, count blocks by type name
//!    (bhkCollisionObject, bhkRigidBody[T], shape families).
//! B. COVERAGE BY BASE TYPE: refs whose NIF has a
//!    bhkCollisionObject vs not, grouped by base record type.
//! C. PORCH / BRIDGE / STAIRS: refs near the inn porch spawn and
//!    the bridge, plus EDID/path matches for bridge, walkway,
//!    porch, stairs — their NIFs' bhk block lists in detail.
//!
//! Usage: cargo run -p dovah-app --example collision_audit
//! (run from the workspace root; reads test-data/)

use dovah_bsa::BsaArchive;
use dovah_esm::{Group, GroupType, Plugin, Record};
use dovah_nif::NifFile;
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

const FLAG_INITIALLY_DISABLED: u32 = 0x00000800;
const FLAG_IS_MARKER: u32 = 0x00000020;

// Exterior spawn (inn porch), Skyrim units — from the XTEL record.
const SPAWN: [f32; 3] = [23259.8, -44476.2, -8.3];

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
    base_type: String,
    base_edid: String,
    model_path: String,
    position: [f32; 3],
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
        let Some((position, _rotation)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data))
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
            base_type: base.type_str(),
            base_edid: base.edid().unwrap_or_default(),
            model_path: format!("meshes\\{model}"),
            position,
        });
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

    // Parse each unique NIF once.
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
    let parsed = nif_cache.values().filter(|n| n.is_some()).count();
    println!(
        "unique NIFs: {}, parsed: {}, missing/unparseable: {}",
        unique_paths.len(),
        parsed,
        unique_paths.len() - parsed
    );

    // ---- A. bhk census ----
    println!("\n=== A. bhk BLOCK CENSUS (unique NIFs) ===");
    let mut type_nifs: BTreeMap<String, usize> = BTreeMap::new();
    let mut type_blocks: BTreeMap<String, usize> = BTreeMap::new();
    let mut nifs_with_collision = 0usize;
    for nif in nif_cache.values().flatten() {
        let mut seen: HashSet<String> = HashSet::new();
        for block in &nif.blocks {
            let t = block.type_name();
            if !t.starts_with("bhk") {
                continue;
            }
            *type_blocks.entry(t.to_string()).or_insert(0) += 1;
            seen.insert(t.to_string());
        }
        if seen.contains("bhkCollisionObject") {
            nifs_with_collision += 1;
        }
        for t in seen {
            *type_nifs.entry(t).or_insert(0) += 1;
        }
    }
    println!("NIFs with bhkCollisionObject: {nifs_with_collision} / {parsed}");
    println!("{:<34} {:>6} {:>8}", "block type", "nifs", "blocks");
    for (t, n) in &type_nifs {
        println!("{:<34} {:>6} {:>8}", t, n, type_blocks[t]);
    }

    // ---- B. coverage by base type ----
    println!("\n=== B. COLLISION COVERAGE BY BASE TYPE (refs) ===");
    let mut cov: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (where_, refs) in [("EXT", &ext_refs), ("INN", &inn_refs)] {
        for pref in refs {
            let has = nif_cache
                .get(&pref.model_path)
                .and_then(|n| n.as_ref())
                .map(|nif| {
                    nif.blocks
                        .iter()
                        .any(|b| b.type_name() == "bhkCollisionObject")
                })
                .unwrap_or(false);
            let entry = cov
                .entry(format!("{where_} {}", pref.base_type))
                .or_insert((0, 0));
            entry.1 += 1;
            if has {
                entry.0 += 1;
            }
        }
    }
    println!("{:<16} {:>6} {:>6}", "where+type", "with", "total");
    for (k, (with, total)) in &cov {
        println!("{:<16} {:>6} {:>6}", k, with, total);
    }

    // ---- C. porch / bridge / stairs ----
    println!("\n=== C. PORCH / BRIDGE / STAIRS ===");
    let near_spawn = |p: [f32; 3]| {
        let dx = p[0] - SPAWN[0];
        let dy = p[1] - SPAWN[1];
        (dx * dx + dy * dy).sqrt() / 70.0
    };
    let mut shown: HashSet<String> = HashSet::new();
    for pref in &ext_refs {
        let edid_l = pref.base_edid.to_lowercase();
        let path_l = pref.model_path.to_lowercase();
        let keyword = [
            "bridge",
            "walkway",
            "porch",
            "stair",
            "ramp",
            "mill",
            "boardwalk",
        ]
        .iter()
        .any(|k| edid_l.contains(k) || path_l.contains(k));
        let near = near_spawn(pref.position) < 25.0;
        if !keyword && !near {
            continue;
        }
        if !shown.insert(pref.model_path.clone()) {
            continue;
        }
        let Some(Some(nif)) = nif_cache.get(&pref.model_path) else {
            println!(
                "  ref 0x{:08x} {} {} ({}m) — NIF missing/unparseable",
                pref.form_id,
                pref.base_edid,
                pref.model_path,
                near_spawn(pref.position) as i32
            );
            continue;
        };
        let bhk: Vec<&str> = nif
            .blocks
            .iter()
            .map(|b| b.type_name())
            .filter(|t| t.starts_with("bhk"))
            .collect();
        println!(
            "  ref 0x{:08x} {} {} ({}m) — {} bhk blocks: {}",
            pref.form_id,
            pref.base_edid,
            pref.model_path,
            near_spawn(pref.position) as i32,
            bhk.len(),
            if bhk.is_empty() {
                "(none)".to_string()
            } else {
                bhk.join(", ")
            }
        );
    }
}
