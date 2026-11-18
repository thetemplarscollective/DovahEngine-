//! Secret-door probe: which placed objects would the EXTENDED
//! hinge rule catch?
//!
//! M15's `is_hinge_node` matches `Door` + digits, and the scene
//! builder treats ANY model with a hinge-node sub-mesh as a
//! swinging door. The secret door needs `LeftDoor`/`RightDoor`
//! added to that rule — but only if no NON-door base (a wardrobe,
//! a chest) carries those nodes in its NIF, or the extension
//! would turn containers into doors and break M17's USE priority
//! (door > container). This sweep lists every ref in the inn +
//! the Riverwood 3x3 whose NIF has a mesh under a `Door*`,
//! `LeftDoor*` or `RightDoor*` node, grouped by base type.
//!
//! Usage: cargo run -p dovah-app --example hinge_sweep

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

/// The EXTENDED hinge rule under test: `Door`, `LeftDoor`,
/// `RightDoor`, each optionally followed by digits only.
fn is_hinge_node(name: &str) -> bool {
    for base in ["Door", "LeftDoor", "RightDoor"] {
        if name == base {
            return true;
        }
        if name.len() > base.len()
            && name.starts_with(base)
            && name[base.len()..].chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let index = CellIndex::build(&plugin);
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    let tamriel = tamriel_cells(&plugin);

    // All refs in the inn + the Riverwood 3x3 (same walk as
    // door_hinge, but every base type, not just DOOR).
    let mut refs: Vec<&Record> = Vec::new();
    if let Some(inn) = plugin
        .all_records()
        .into_iter()
        .find(|r| r.type_str() == "CELL" && r.edid().as_deref() == Some(INN_EDID))
    {
        refs.extend(index.children_of(inn.form_id));
    }
    let min_x = (CENTRE.0 - 1) as f32 * CELL_SPAN;
    let max_x = (CENTRE.0 + 2) as f32 * CELL_SPAN;
    let min_y = (CENTRE.1 - 1) as f32 * CELL_SPAN;
    let max_y = (CENTRE.1 + 2) as f32 * CELL_SPAN;
    for r in index.children_of(PERSISTENT_CELL_FORM_ID) {
        if r.type_str() != "REFR" {
            continue;
        }
        let inside = r
            .field(b"DATA")
            .and_then(|f| parse_placement(&f.data))
            .map(|(pos, _)| pos[0] >= min_x && pos[0] < max_x && pos[1] >= min_y && pos[1] < max_y)
            .unwrap_or(false);
        if inside {
            refs.push(r);
        }
    }
    for gx in CENTRE.0 - 1..=CENTRE.0 + 1 {
        for gy in CENTRE.1 - 1..=CENTRE.1 + 1 {
            if let Some(cell) = tamriel.get(&(gx, gy)) {
                refs.extend(index.children_of(cell.form_id));
            }
        }
    }
    refs.sort_by_key(|r| r.form_id);
    refs.dedup_by_key(|r| r.form_id);
    println!("refs (inn + Riverwood 3x3): {}", refs.len());

    let mut archives = Archives::open_all(Path::new(DATA_DIR));
    // NIF path -> hinge parent nodes found (empty = no hinge).
    let mut nif_cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut by_base_type: HashMap<String, usize> = HashMap::new();
    let mut non_door_hits: Vec<(u32, String, String, String, Vec<String>)> = Vec::new();

    for r in &refs {
        if r.type_str() != "REFR" {
            continue;
        }
        if r.flags & FLAG_INITIALLY_DISABLED != 0 || r.flags & FLAG_IS_MARKER != 0 {
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
        let nif_path = format!("meshes\\{model}");
        let hinges = nif_cache.entry(nif_path.clone()).or_insert_with(|| {
            let mut found: HashSet<String> = HashSet::new();
            if let Some(bytes) = archives.read_file(&nif_path) {
                if let Ok(nif) = NifFile::from_bytes(&bytes) {
                    for mesh in nif.meshes() {
                        if is_hinge_node(&mesh.parent_node) {
                            found.insert(mesh.parent_node.clone());
                        }
                    }
                }
            }
            let mut found: Vec<String> = found.into_iter().collect();
            found.sort();
            found
        });
        if hinges.is_empty() {
            continue;
        }
        let base_type = base.type_str().to_string();
        *by_base_type.entry(base_type.clone()).or_insert(0) += 1;
        println!(
            "  ref 0x{:08x} base {} {} ({}) hinges={:?}",
            r.form_id,
            base_type,
            base.edid().unwrap_or_default(),
            nif_path,
            hinges
        );
        if base_type != "DOOR" {
            non_door_hits.push((
                r.form_id,
                base_type,
                base.edid().unwrap_or_default(),
                nif_path,
                hinges.clone(),
            ));
        }
    }
    println!("\nhinge refs by base type: {by_base_type:?}");
    if non_door_hits.is_empty() {
        println!("NON-DOOR HITS: none — the extended rule only catches DOOR bases here.");
    } else {
        println!("NON-DOOR HITS ({}):", non_door_hits.len());
        for (form_id, base_type, edid, path, hinges) in &non_door_hits {
            println!("  ref 0x{form_id:08x} {base_type} {edid} {path} {hinges:?}");
        }
    }
}
