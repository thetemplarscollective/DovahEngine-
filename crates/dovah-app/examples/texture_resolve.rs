//! M10 probe: re-resolve the Sleeping Giant Inn + Riverwood
//! exterior diffuse textures against ALL texture BSAs on hand.
//!
//! Mirrors the app's own counting (scene.rs `real_material`):
//! unique diffuse paths from drawable sub-meshes (non-empty
//! vertices + indices) of real-mesh objects. Actors are skipped
//! (capsule placeholders, no textures); terrain is skipped
//! (VCLR vertex colors, no textures).
//!
//! Prints, for the inn and the exterior separately: resolved /
//! missing counts and the full list of still-missing texture
//! paths. A missing texture whose path exists in NO archive on
//! hand tells us which BSA is still needed (e.g. Textures9).
//!
//! Also reports texture paths that exist in MORE than one archive
//! — the app's provider is first-wins, so duplicates matter.
//!
//! Usage: cargo run -p dovah-app --example texture_resolve

use dovah_bsa::BsaArchive;
use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::Path;

const DATA_DIR: &str = "test-data";
const ESM_PATH: &str = "test-data/Skyrim.esm";
const INN_EDID: &str = "RiverwoodSleepingGiantInn";
const CELL_SIZE_UNITS: f32 = 4096.0;
const CENTRE: (i32, i32) = (5, -11);
const PERSISTENT_CELL_FORM_ID: u32 = 0x00000d74;

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

fn is_interior(cell: &Record) -> bool {
    cell.field(b"DATA")
        .and_then(|f| f.data.first())
        .map(|b| b & 0x01 != 0)
        .unwrap_or(false)
}

/// One pass over the group tree: cell form ID -> its child
/// records. (Same pattern as riverwood_probe.rs.)
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

struct Archives {
    archives: Vec<BsaArchive<File>>,
    /// Normalized path -> archive indices holding it (all of
    /// them, so duplicates are visible).
    index: HashMap<String, Vec<usize>>,
    names: Vec<String>,
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
            names: Vec::new(),
        };
        for path in paths {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match BsaArchive::open(&path) {
                Ok(archive) => {
                    let idx = out.archives.len();
                    for stored in archive.list() {
                        out.index.entry(normalize(stored)).or_default().push(idx);
                    }
                    out.names.push(name);
                    out.archives.push(archive);
                }
                Err(e) => eprintln!("warning: could not open {}: {e}", path.display()),
            }
        }
        out
    }

    fn read_file(&mut self, path: &str) -> Option<Vec<u8>> {
        let idx = *self.index.get(&normalize(path))?.first()?;
        if let Ok(bytes) = self.archives[idx].extract(path) {
            return Some(bytes);
        }
        // Stored paths may differ in case/separators from the
        // query; fall back to the stored spelling.
        let stored: String = self.archives[idx]
            .list()
            .into_iter()
            .find(|s| normalize(s) == normalize(path))?
            .to_string();
        self.archives[idx].extract(&stored).ok()
    }

    fn contains(&self, path: &str) -> bool {
        self.index.contains_key(&normalize(path))
    }
}

/// What a diffuse-path collection found: the unique texture
/// paths, the NIFs that could not be found, how many NIFs failed
/// to parse, and (nif, diffuse) pairs for provenance.
struct DiffuseCollection {
    paths: HashSet<String>,
    nifs_not_found: Vec<String>,
    parse_failed: usize,
    provenance: Vec<(String, String)>,
}

/// Collect unique diffuse texture paths for a set of placed refs.
fn diffuse_paths(
    refs: &[&Record],
    by_form_id: &HashMap<u32, &Record>,
    archives: &mut Archives,
    nif_cache: &mut HashMap<String, Vec<String>>,
) -> DiffuseCollection {
    let mut out = DiffuseCollection {
        paths: HashSet::new(),
        nifs_not_found: Vec::new(),
        parse_failed: 0,
        provenance: Vec::new(),
    };
    for r in refs {
        if r.type_str() != "REFR" {
            continue; // ACHR actors are placeholders — no textures.
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
        if let Some(cached) = nif_cache.get(&nif_path) {
            for p in cached {
                out.paths.insert(p.clone());
            }
            continue;
        }
        let mut found_paths: Vec<String> = Vec::new();
        match archives.read_file(&nif_path) {
            Some(bytes) => match dovah_nif::NifFile::from_bytes(&bytes) {
                Ok(nif) => {
                    for sub in nif.meshes() {
                        // Mirror convert::nif_mesh_to_bevy: only
                        // drawable sub-meshes get a material.
                        if sub.vertices.is_empty() || sub.indices.is_empty() {
                            continue;
                        }
                        if let Some(d) = &sub.textures.diffuse {
                            found_paths.push(d.clone());
                            out.paths.insert(d.clone());
                            out.provenance.push((nif_path.clone(), d.clone()));
                        }
                    }
                }
                Err(_) => out.parse_failed += 1,
            },
            None => out.nifs_not_found.push(nif_path.clone()),
        }
        nif_cache.insert(nif_path, found_paths);
    }
    out
}

/// Strip a build-machine prefix: NIFs sometimes reference
/// textures by their original build path
/// (`skyrimhd\build\pc\data\textures\...`) instead of the game
/// path (`textures\...`). The game strips everything up to the
/// `data\` dir; do the same.
fn strip_build_prefix(normalized: &str) -> &str {
    match normalized.rfind("/data/") {
        Some(i) => &normalized[i + 6..],
        None => normalized,
    }
}

fn report(
    label: &str,
    paths: &HashSet<String>,
    archives: &Archives,
    not_found: &[String],
    parse_failed: usize,
    provenance: &[(String, String)],
) -> Vec<String> {
    let mut resolved = 0usize;
    let mut missing: Vec<String> = Vec::new();
    let mut fixed_by_strip: Vec<String> = Vec::new();
    for p in paths {
        if archives.contains(p) {
            resolved += 1;
        } else if archives.contains(strip_build_prefix(&normalize(p))) {
            // Resolves once the build-machine prefix is stripped.
            resolved += 1;
            fixed_by_strip.push(p.clone());
        } else {
            missing.push(p.clone());
        }
    }
    missing.sort();
    fixed_by_strip.sort();
    println!("\n=== {label} ===");
    println!(
        "unique diffuse textures: {} — resolved {} / missing {} ({} via prefix-strip)",
        paths.len(),
        resolved,
        missing.len(),
        fixed_by_strip.len()
    );
    println!(
        "nifs not found: {}, nifs failed to parse: {parse_failed}",
        not_found.len()
    );
    let mut nf_sorted = not_found.to_vec();
    nf_sorted.sort();
    nf_sorted.dedup();
    for n in &nf_sorted {
        println!("  nif not found: {n}");
    }
    if !fixed_by_strip.is_empty() {
        println!("resolve only after stripping the build prefix:");
        for m in &fixed_by_strip {
            println!("  {m}");
        }
    }
    if !missing.is_empty() {
        println!("still missing (even after prefix-strip):");
        for m in &missing {
            let users: Vec<&str> = provenance
                .iter()
                .filter(|(_, d)| d == m)
                .map(|(n, _)| n.as_str())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            println!("  {m}  [from: {}]", users.join(", "));
        }
    }
    missing
}

fn main() {
    let mut archives = Archives::open_all(Path::new(DATA_DIR));
    println!("archives opened: {}", archives.names.len());
    for n in &archives.names {
        println!("  {n}");
    }
    println!("indexed files: {}", archives.index.len());

    // Duplicate texture paths across archives (provider is
    // first-wins, so a duplicate means the lower archive wins).
    let mut dups: Vec<(&String, Vec<&String>)> = Vec::new();
    for (path, holders) in &archives.index {
        if holders.len() > 1 && path.ends_with(".dds") {
            let names: Vec<&String> = holders.iter().map(|i| &archives.names[*i]).collect();
            dups.push((path, names));
        }
    }
    dups.sort_by_key(|(p, _)| (*p).clone());
    println!("\nduplicate .dds paths across archives: {}", dups.len());
    for (p, names) in dups.iter().take(20) {
        println!("  {p} in {names:?}");
    }

    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let index = CellIndex::build(&plugin);
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }

    // Exterior grid map by median ref position (same as M9).
    let mut exterior_by_grid: HashMap<(i32, i32), &Record> = HashMap::new();
    for record in plugin.all_records() {
        if record.type_str() != "CELL" || is_interior(record) {
            continue;
        }
        let recs = index.children_of(record.form_id);
        let mut xs: Vec<f32> = Vec::new();
        let mut ys: Vec<f32> = Vec::new();
        for r in recs {
            if r.type_str() != "REFR" && r.type_str() != "ACHR" {
                continue;
            }
            if let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) {
                xs.push(pos[0]);
                ys.push(pos[1]);
            }
        }
        if xs.is_empty() {
            continue;
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let gx = (xs[xs.len() / 2] / CELL_SIZE_UNITS).floor() as i32;
        let gy = (ys[ys.len() / 2] / CELL_SIZE_UNITS).floor() as i32;
        exterior_by_grid.insert((gx, gy), record);
    }

    let mut nif_cache: HashMap<String, Vec<String>> = HashMap::new();

    // --- Inn ---
    let inn = plugin
        .all_records()
        .into_iter()
        .find(|r| r.type_str() == "CELL" && r.edid().as_deref() == Some(INN_EDID))
        .expect("inn cell");
    let inn_refs = index.children_of(inn.form_id);
    println!("\ninn cell refs: {}", inn_refs.len());
    let inn = diffuse_paths(inn_refs, &by_form_id, &mut archives, &mut nif_cache);
    let inn_missing = report(
        "Inn (RiverwoodSleepingGiantInn)",
        &inn.paths,
        &archives,
        &inn.nifs_not_found,
        inn.parse_failed,
        &inn.provenance,
    );

    // --- Exterior: 3x3 block + persistent refs in the area ---
    let mut ext_refs: Vec<&Record> = Vec::new();
    for dy in -1..=1 {
        for dx in -1..=1 {
            let g = (CENTRE.0 + dx, CENTRE.1 + dy);
            if let Some(cell) = exterior_by_grid.get(&g) {
                ext_refs.extend(index.children_of(cell.form_id));
            } else {
                println!("grid {g:?}: NO CELL");
            }
        }
    }
    let min_x = (CENTRE.0 - 1) as f32 * CELL_SIZE_UNITS;
    let max_x = (CENTRE.0 + 2) as f32 * CELL_SIZE_UNITS;
    let min_y = (CENTRE.1 - 1) as f32 * CELL_SIZE_UNITS;
    let max_y = (CENTRE.1 + 2) as f32 * CELL_SIZE_UNITS;
    let mut persist_in_area = 0usize;
    for r in index.children_of(PERSISTENT_CELL_FORM_ID) {
        if r.type_str() != "REFR" && r.type_str() != "ACHR" {
            continue;
        }
        if let Some((pos, _)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data)) {
            if pos[0] >= min_x && pos[0] < max_x && pos[1] >= min_y && pos[1] < max_y {
                ext_refs.push(r);
                persist_in_area += 1;
            }
        }
    }
    println!(
        "\nexterior refs: {} (incl. {persist_in_area} persistent in area)",
        ext_refs.len()
    );
    let ext = diffuse_paths(&ext_refs, &by_form_id, &mut archives, &mut nif_cache);
    let ext_missing = report(
        "Exterior (Riverwood 3x3)",
        &ext.paths,
        &archives,
        &ext.nifs_not_found,
        ext.parse_failed,
        &ext.provenance,
    );

    println!("\n=== SUMMARY ===");
    println!(
        "inn: {}/{} resolve; exterior: {}/{} resolve",
        inn.paths.len() - inn_missing.len(),
        inn.paths.len(),
        ext.paths.len() - ext_missing.len(),
        ext.paths.len()
    );
}
