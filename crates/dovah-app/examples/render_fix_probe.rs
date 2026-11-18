//! Rendering-fix data probe (v0.4.13): one ESM pass for the
//! remaining diagnosis questions.
//!
//! Part 1 — SVEN'S DOOR: the exit door 0x0001cbaf swings open but
//! never transitions; the inn door 0x00013419 works. Dump both
//! doors (+ Sven's entry 0x0001cbb0, inn exterior 0x00013424):
//! base, XTEL, position, rotation, OBND.
//! Part 2 — GREEN BOX: the 5 exterior placeholders — which refs,
//! which bases, what MODL bytes (the texture_resolve probe found
//! garbage MODL paths), where they sit vs the workbench
//! (0x0003550d) and grindstone (0x0006f947).
//! Part 3 — FIRE PIT: inn cell refs whose base EDID mentions
//! fire/pit/hearth/coals/flame — the rainbow fire pit's NIF.
//! Part 4 — BOTTLE: inn cell refs whose base MODL mentions
//! "bottle" — the white-disc bottle's NIF.
//!
//! Usage: cargo run -p dovah-app --example render_fix_probe

use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::HashMap;

fn cell_children<'a>(plugin: &'a Plugin, cell_id: u32) -> Vec<&'a Record> {
    let mut out = Vec::new();
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        if matches!(
            g.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        ) && g.label_form_id() == Some(cell_id)
        {
            out.extend(g.records());
        }
        stack.extend(g.subgroups());
    }
    out
}

fn form_field(record: &Record, code: &[u8; 4]) -> Option<u32> {
    record.field(code).and_then(|f| {
        if f.data.len() == 4 {
            Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
        } else {
            None
        }
    })
}

fn placement(record: &Record) -> ([f32; 3], [f32; 3]) {
    record
        .field(b"DATA")
        .and_then(|f| {
            if f.data.len() == 24 {
                let g = |i: usize| f32::from_le_bytes(f.data[i * 4..i * 4 + 4].try_into().unwrap());
                Some(([g(0), g(1), g(2)], [g(3), g(4), g(5)]))
            } else {
                None
            }
        })
        .unwrap_or(([f32::NAN; 3], [f32::NAN; 3]))
}

fn parse_xtel(data: &[u8]) -> Option<(u32, [f32; 3], [f32; 3], u32)> {
    if data.len() != 32 {
        return None;
    }
    let u = |i: usize| u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    let f = |i: usize| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    Some((u(0), [f(1), f(2), f(3)], [f(4), f(5), f(6)], u(7)))
}

fn obnd(base: &Record) -> Option<([i16; 3], [i16; 3])> {
    base.field(b"OBND").and_then(|f| {
        if f.data.len() == 12 {
            let g = |i: usize| i16::from_le_bytes(f.data[i * 2..i * 2 + 2].try_into().unwrap());
            Some(([g(0), g(1), g(2)], [g(3), g(4), g(5)]))
        } else {
            None
        }
    })
}

fn modl_bytes(base: &Record) -> Option<Vec<u8>> {
    base.field(b"MODL").map(|f| f.data.clone())
}

fn bevy(pos: [f32; 3]) -> [f32; 3] {
    // Skyrim (x,y,z) units -> Bevy metres: (x, z, -y) / 70.
    [pos[0] / 70.0, pos[2] / 70.0, -pos[1] / 70.0]
}

fn main() {
    let plugin = Plugin::open("test-data/Skyrim.esm").expect("open Skyrim.esm");
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }

    // ---- Part 1: doors ----
    println!("=== PART 1: DOORS ===");
    for ref_id in [0x0001cbafu32, 0x0001cbb0, 0x00013419, 0x00013424] {
        let Some(r) = by_form_id.get(&ref_id) else {
            println!("ref 0x{ref_id:08x}: NOT FOUND");
            continue;
        };
        let (pos, rot) = placement(r);
        let b = bevy(pos);
        let base_id = form_field(r, b"NAME").unwrap_or(0);
        let base = by_form_id.get(&base_id);
        let (btype, bedid, bmodl) = match base {
            Some(b) => (
                b.type_str(),
                b.edid().unwrap_or_default(),
                b.field(b"MODL")
                    .map(|f| String::from_utf8_lossy(&f.data).trim_end_matches('\0').to_string())
                    .unwrap_or_default(),
            ),
            None => ("?".to_string(), String::new(), String::new()),
        };
        let ob = base.and_then(obnd);
        println!(
            "ref 0x{ref_id:08x} ({}): base 0x{base_id:08x} [{} {}] MODL={} pos_sky=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3}) bevy=({:.2},{:.2},{:.2}) obnd={ob:?}",
            r.edid().unwrap_or_default(),
            btype,
            bedid,
            bmodl,
            pos[0],
            pos[1],
            pos[2],
            rot[0],
            rot[1],
            rot[2],
            b[0],
            b[1],
            b[2]
        );
        match r.field(b"XTEL").and_then(|f| parse_xtel(&f.data)) {
            Some((dest, dpos, drot, tail)) => {
                let db = bevy(dpos);
                let dest_edid = by_form_id
                    .get(&dest)
                    .and_then(|d| d.edid().ok())
                    .unwrap_or_default();
                println!(
                    "  XTEL -> dest ref 0x{dest:08x} ({dest_edid}) pos=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3}) bevy=({:.2},{:.2},{:.2}) tail=0x{tail:08x}",
                    dpos[0], dpos[1], dpos[2], drot[0], drot[1], drot[2], db[0], db[1], db[2]
                );
            }
            None => println!(
                "  NO XTEL (field len {:?})",
                r.field(b"XTEL").map(|f| f.data.len())
            ),
        }
    }
    // Is Sven's exit door a child of Sven's house cell? And which
    // cell is each door ref in?
    for (cell_id, name) in [(0x0001cb84u32, "SvensHouse"), (0x000133c6u32, "Inn")] {
        let kids = cell_children(&plugin, cell_id);
        let doors: Vec<u32> = kids
            .iter()
            .filter(|r| r.field(b"XTEL").is_some())
            .map(|r| r.form_id)
            .collect();
        println!("cell {name} (0x{cell_id:08x}): {} children, XTEL refs: {doors:08x?}", kids.len());
    }

    // ---- Part 2: exterior placeholders + smithy ----
    println!("\n=== PART 2: EXTERIOR PLACEHOLDERS + SMITHY ===");
    // Riverwood 3x3 block cells from TAMRIEL's WorldChildren.
    let mut cells: Vec<(i32, i32, u32)> = Vec::new();
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        if g.group_type == GroupType::WorldChildren && g.label_form_id() == Some(0x0000003c) {
            let mut inner: Vec<&Group> = vec![g];
            while let Some(h) = inner.pop() {
                for record in h.records() {
                    if record.type_str() != "CELL" {
                        continue;
                    }
                    let Some(f) = record.field(b"XCLC") else {
                        continue;
                    };
                    if f.data.len() < 8 {
                        continue;
                    }
                    let x = i32::from_le_bytes(f.data[0..4].try_into().unwrap());
                    let y = i32::from_le_bytes(f.data[4..8].try_into().unwrap());
                    if (4..=6).contains(&x) && (-12..=-10).contains(&y) {
                        cells.push((x, y, record.form_id));
                    }
                }
                inner.extend(h.subgroups());
            }
        }
        stack.extend(g.subgroups());
    }
    let mut refs: Vec<&Record> = Vec::new();
    for (_, _, id) in &cells {
        refs.extend(cell_children(&plugin, *id));
    }
    refs.extend(cell_children(&plugin, 0x00000d74));
    println!("exterior refs scanned: {}", refs.len());
    for r in &refs {
        let Some(base_id) = form_field(r, b"NAME") else {
            continue;
        };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        // A placeholder ref: base has a MODL that is garbage
        // (non-printable bytes / not ending in .nif) or no MODL
        // at all while the base type normally has a model.
        let modl = modl_bytes(base);
        let garbage = match &modl {
            None => false,
            Some(b) => {
                let s = String::from_utf8_lossy(b);
                let t = s.trim_end_matches('\0');
                !t.to_lowercase().ends_with(".nif")
                    || t.chars().any(|c| (c as u32) < 0x20)
            }
        };
        if garbage {
            let (pos, _rot) = placement(r);
            let b = bevy(pos);
            println!(
                "  GARBAGE-MODL ref 0x{:08x}: base 0x{base_id:08x} [{} {}] MODL bytes={:?} bevy=({:.2},{:.2},{:.2}) obnd={:?}",
                r.form_id,
                base.type_str(),
                base.edid().unwrap_or_default(),
                modl.unwrap(),
                b[0],
                b[1],
                b[2],
                obnd(base)
            );
        }
    }
    // Smithy anchors.
    for ref_id in [0x0003550du32, 0x0006f947] {
        if let Some(r) = by_form_id.get(&ref_id) {
            let (pos, _rot) = placement(r);
            let b = bevy(pos);
            let base_id = form_field(r, b"NAME").unwrap_or(0);
            let bedid = by_form_id
                .get(&base_id)
                .and_then(|x| x.edid().ok())
                .unwrap_or_default();
            println!(
                "  anchor ref 0x{ref_id:08x} ({bedid}): bevy=({:.2},{:.2},{:.2})",
                b[0], b[1], b[2]
            );
        }
    }

    // ---- Part 3: inn fire pit ----
    println!("\n=== PART 3: INN FIRE REFS ===");
    let inn = cell_children(&plugin, 0x000133c6);
    for r in &inn {
        let Some(base_id) = form_field(r, b"NAME") else {
            continue;
        };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        let edid = base.edid().unwrap_or_default().to_lowercase();
        if edid.contains("fire") || edid.contains("pit") || edid.contains("hearth") || edid.contains("coal") || edid.contains("flame") || edid.contains("ember") {
            let (pos, _rot) = placement(r);
            let b = bevy(pos);
            let m = base
                .field(b"MODL")
                .map(|f| String::from_utf8_lossy(&f.data).trim_end_matches('\0').to_string())
                .unwrap_or_default();
            println!(
                "  ref 0x{:08x}: base 0x{base_id:08x} [{} {}] MODL={} bevy=({:.2},{:.2},{:.2})",
                r.form_id,
                base.type_str(),
                base.edid().unwrap_or_default(),
                m,
                b[0],
                b[1],
                b[2]
            );
        }
    }

    // ---- Part 4: inn bottles ----
    println!("\n=== PART 4: INN BOTTLE REFS ===");
    for r in &inn {
        let Some(base_id) = form_field(r, b"NAME") else {
            continue;
        };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        let m = base
            .field(b"MODL")
            .map(|f| String::from_utf8_lossy(&f.data).trim_end_matches('\0').to_string())
            .unwrap_or_default();
        if m.to_lowercase().contains("bottle") {
            let (pos, _rot) = placement(r);
            let b = bevy(pos);
            println!(
                "  ref 0x{:08x}: base 0x{base_id:08x} [{} {}] MODL={} bevy=({:.2},{:.2},{:.2})",
                r.form_id,
                base.type_str(),
                base.edid().unwrap_or_default(),
                m,
                b[0],
                b[1],
                b[2]
            );
        }
    }
}
