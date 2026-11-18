//! M15 probe: where is a door panel's hinge?
//!
//! A swinging door rotates around its hinge edge. This probe
//! proves the hinge location from the data instead of guessing
//! it: for every DOOR ref in the inn + the Riverwood 3x3 block
//! it prints the base record's OBND and the door NIF's
//! LOCAL-space vertex bounds (shape transforms applied, ref
//! transform NOT applied). If the panel's X range starts at ~0
//! (panel extends from the origin in +X or -X), the NIF origin
//! IS the hinge and the swing is a pure rotation about the ref
//! position. If the X range is centred on 0, the hinge is a
//! panel edge offset from the origin and the swing needs the
//! offset.
//!
//! Usage: cargo run -p dovah-app --example door_hinge

use dovah_bsa::BsaArchive;
use dovah_esm::{Group, GroupType, Plugin, Record};
use dovah_nif::NifFile;
use std::collections::HashMap;
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

struct DoorRef {
    form_id: u32,
    base_edid: String,
    model_path: String,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: f32,
    obnd: Option<([i16; 3], [i16; 3])>,
    has_xtel: bool,
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
        let obnd = base.field(b"OBND").and_then(|f| {
            if f.data.len() != 12 {
                return None;
            }
            let v = |i: usize| i16::from_le_bytes(f.data[i * 2..i * 2 + 2].try_into().unwrap());
            Some(([v(0), v(1), v(2)], [v(3), v(4), v(5)]))
        });
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
            obnd,
            has_xtel: r.field(b"XTEL").is_some(),
        });
    }
}

/// Row-major 3x3 matrix times a vector.
fn mat_vec(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let index = CellIndex::build(&plugin);
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    let tamriel = tamriel_cells(&plugin);

    let mut door_refs: Vec<DoorRef> = Vec::new();
    if let Some(inn) = plugin
        .all_records()
        .into_iter()
        .find(|r| r.type_str() == "CELL" && r.edid().as_deref() == Some(INN_EDID))
    {
        collect_doors(index.children_of(inn.form_id), &by_form_id, &mut door_refs);
    }
    // Persistent refs inside the 3x3 block (exterior doors live
    // there, not in the geographic cells).
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
    collect_doors(&persist, &by_form_id, &mut door_refs);
    // Geographic cells' own children (a few doors sit there).
    for gx in CENTRE.0 - 1..=CENTRE.0 + 1 {
        for gy in CENTRE.1 - 1..=CENTRE.1 + 1 {
            if let Some(cell) = tamriel.get(&(gx, gy)) {
                collect_doors(index.children_of(cell.form_id), &by_form_id, &mut door_refs);
            }
        }
    }
    // De-dup (persistent refs can appear in both walks).
    door_refs.sort_by_key(|d| d.form_id);
    door_refs.dedup_by_key(|d| d.form_id);
    println!("door refs (inn + Riverwood 3x3): {}", door_refs.len());

    let mut archives = Archives::open_all(Path::new(DATA_DIR));
    let mut nif_cache: HashMap<String, Option<String>> = HashMap::new();
    for door in &door_refs {
        println!(
            "\ndoor 0x{:08x} {} xtel={} scale={:.3} ref pos=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3})",
            door.form_id,
            door.base_edid,
            door.has_xtel,
            door.scale,
            door.position[0],
            door.position[1],
            door.position[2],
            door.rotation[0],
            door.rotation[1],
            door.rotation[2]
        );
        match door.obnd {
            Some((min, max)) => println!(
                "  OBND: min=({},{},{}) max=({},{},{}) size=({}x{}x{})u = ({:.2}x{:.2}x{:.2})m",
                min[0],
                min[1],
                min[2],
                max[0],
                max[1],
                max[2],
                max[0] - min[0],
                max[1] - min[1],
                max[2] - min[2],
                (max[0] - min[0]) as f32 / 70.0,
                (max[1] - min[1]) as f32 / 70.0,
                (max[2] - min[2]) as f32 / 70.0
            ),
            None => println!("  OBND: none"),
        }
        println!("  model: {}", door.model_path);
        let cached = nif_cache
            .entry(door.model_path.clone())
            .or_insert_with(|| {
                let Some(bytes) = archives.read_file(&door.model_path) else {
                    return Some("NIF not found".to_string());
                };
                let Ok(nif) = NifFile::from_bytes(&bytes) else {
                    return Some("NIF parse failed".to_string());
                };
                let mut out = String::new();
                let mut omin = [f32::INFINITY; 3];
                let mut omax = [f32::NEG_INFINITY; 3];
                let mut nshapes = 0;
                for mesh in nif.meshes() {
                    if mesh.vertices.is_empty() || mesh.indices.is_empty() {
                        continue;
                    }
                    nshapes += 1;
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
                    for i in 0..3 {
                        omin[i] = omin[i].min(min[i]);
                        omax[i] = omax[i].max(max[i]);
                    }
                    out.push_str(&format!(
                        "    shape '{}': parent='{}' hinge=({:.1},{:.1},{:.1}) local min=({:.1},{:.1},{:.1}) max=({:.1},{:.1},{:.1}) size=({:.2}x{:.2}x{:.2})m\n",
                        mesh.name,
                        mesh.parent_node,
                        mesh.parent_transform.translation[0],
                        mesh.parent_transform.translation[1],
                        mesh.parent_transform.translation[2],
                        min[0],
                        min[1],
                        min[2],
                        max[0],
                        max[1],
                        max[2],
                        (max[0] - min[0]) / 70.0,
                        (max[1] - min[1]) / 70.0,
                        (max[2] - min[2]) / 70.0
                    ));
                }
                if nshapes == 0 {
                    return Some("NIF has no drawable shapes".to_string());
                }
                // Hinge verdict from the OVERALL local bounds.
                let x0 = omin[0].abs() < 2.0;
                let x1 = omax[0].abs() < 2.0;
                let centred = (omin[0] + omax[0]).abs() < 4.0;
                let verdict = if x0 {
                    "HINGE AT ORIGIN (+X edge): panel extends +X from the ref origin"
                } else if x1 {
                    "HINGE AT ORIGIN (-X edge): panel extends -X from the ref origin"
                } else if centred {
                    "CENTRED: panel straddles the origin; hinge is a panel edge, offset needed"
                } else {
                    "OFFSET: neither edge at the origin — hinge offset must be measured"
                };
                // M15: open angle from the NIF's own Open sequence
                // (the NiTransformData whose Z keys start at the
                // rest angle — 0 modulo 2π, since AnimDoor02
                // authors rest as -2π — and end elsewhere; the
                // angle is the DELTA) — the same rule doors.rs
                // uses.
                let at_rest = |v: f32| {
                    let wrapped = v.rem_euclid(2.0 * std::f32::consts::PI);
                    wrapped < 1e-3 || wrapped > 2.0 * std::f32::consts::PI - 1e-3
                };
                let mut open: Option<(f32, f32)> = None;
                for data in nif.transform_data() {
                    if let (Some(first), Some(last)) =
                        (data.z_keys.first(), data.z_keys.last())
                    {
                        if at_rest(first.value) && !at_rest(last.value) {
                            let delta = last.value - first.value;
                            let better = open
                                .map(|(a, _)| delta.abs() > a.abs())
                                .unwrap_or(true);
                            if better {
                                open = Some((delta, last.time));
                            }
                        }
                    }
                }
                match open {
                    Some((angle, duration)) => out.push_str(&format!(
                        "  OPEN ANGLE: {angle:.4} rad ({:.1} deg) over {duration:.2}s\n",
                        angle.to_degrees()
                    )),
                    None => out.push_str("  OPEN ANGLE: none (no parseable Open sequence)\n"),
                }
                out.push_str(&format!(
                    "  OVERALL ({} shapes): local min=({:.1},{:.1},{:.1}) max=({:.1},{:.1},{:.1}) size=({:.2}x{:.2}x{:.2})m\n  HINGE: {verdict}\n",
                    nshapes,
                    omin[0],
                    omin[1],
                    omin[2],
                    omax[0],
                    omax[1],
                    omax[2],
                    (omax[0] - omin[0]) / 70.0,
                    (omax[1] - omin[1]) / 70.0,
                    (omax[2] - omin[2]) / 70.0
                ));
                Some(out)
            });
        match cached {
            Some(text) => print!("{text}"),
            None => println!("  (no NIF data)"),
        }
    }
}
