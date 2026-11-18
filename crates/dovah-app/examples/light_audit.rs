//! M14 probe: audit the Sleeping Giant Inn's lights against the data.
//!
//! Answers three questions with numbers:
//! A. How many REFRs in the inn cell have a LIGH base, and what are
//!    their DATA values (radius, color, flags, falloff, FOV)?
//! B. How many of those LIGH bases carry a MODL (visible mesh) vs
//!    pure light sources — and does the app's loader drop the
//!    model-less ones (the `skipped_no_model` filter in cell.rs
//!    runs before the light parse, so a model-less LIGH never
//!    reaches `spawn_light`)? The loader filter is simulated
//!    inline below (the app's modules are private, so the example
//!    cannot call `load_cell` directly); the M12 inn log already
//!    gives the loader's answer: `lights: 1` — the central
//!    fallback, i.e. zero real LIGH refs survived.
//!
//! Usage: cargo run -p dovah-app --example light_audit

use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::HashMap;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const INN_FORM_ID: u32 = 0x000133c6;

const FLAG_INITIALLY_DISABLED: u32 = 0x00000800;
const FLAG_IS_MARKER: u32 = 0x00800000;

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

fn form_id_from_field(record: &Record, field_type: &[u8; 4]) -> Option<u32> {
    let field = record.field(field_type)?;
    if field.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(field.data[0..4].try_into().unwrap()))
}

fn f32_at(data: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

fn u32_at(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    let index = CellIndex::build(&plugin);
    let children = index.children_of(INN_FORM_ID);
    println!("inn cell children: {}", children.len());

    let mut ligh_refs = 0usize;
    let mut ligh_with_modl = 0usize;
    let mut ligh_disabled = 0usize;
    let mut ligh_marker = 0usize;
    let mut ligh_would_survive = 0usize;
    let mut by_base_type: HashMap<String, usize> = HashMap::new();
    for r in children {
        if r.type_str() != "REFR" && r.type_str() != "ACHR" {
            continue;
        }
        let Some(base_id) = form_id_from_field(r, b"NAME") else {
            continue;
        };
        let Some(base) = by_form_id.get(&base_id) else {
            continue;
        };
        *by_base_type.entry(base.type_str()).or_insert(0) += 1;
        // ACHR positions (for the Delphine close-up camera):
        // Bevy = (x, z, -y) / 70.
        if r.type_str() == "ACHR" {
            if let Some(f) = r.field(b"DATA") {
                let (x, y, z) = (f32_at(&f.data, 0), f32_at(&f.data, 4), f32_at(&f.data, 8));
                println!(
                    "ACHR 0x{:08x} base {} skyrim ({:.1}, {:.1}, {:.1}) bevy ({:.2}, {:.2}, {:.2})",
                    r.form_id,
                    base.edid().unwrap_or_default(),
                    x,
                    y,
                    z,
                    x / 70.0,
                    z / 70.0,
                    -y / 70.0
                );
            }
        }
        if base.type_str() != "LIGH" {
            continue;
        }
        ligh_refs += 1;
        if r.flags & FLAG_INITIALLY_DISABLED != 0 {
            ligh_disabled += 1;
        }
        if r.flags & FLAG_IS_MARKER != 0 {
            ligh_marker += 1;
        }
        let has_modl = base.field(b"MODL").is_some();
        if has_modl {
            ligh_with_modl += 1;
        }
        let pos = r
            .field(b"DATA")
            .map(|f| (f32_at(&f.data, 0), f32_at(&f.data, 4), f32_at(&f.data, 8)));
        let data = base.field(b"DATA").map(|f| f.data.as_slice());
        let (radius, color, flags, falloff, fov, time) = match data {
            Some(d) if d.len() >= 24 => (
                u32_at(d, 4),
                u32_at(d, 8),
                u32_at(d, 12),
                f32_at(d, 16),
                f32_at(d, 20),
                u32_at(d, 0) as i32,
            ),
            _ => (0, 0, 0, 0.0, 0.0, 0),
        };
        // Simulate the loader filter (cell.rs `append_objects`):
        // disabled -> dropped; marker flag -> dropped; no MODL
        // and not an actor -> dropped as no_model BEFORE the light
        // parse; fx\ model -> dropped as an effect.
        let is_effect = base
            .field(b"MODL")
            .map(|f| f.as_str().to_ascii_lowercase())
            .map(|p| p.starts_with("fx\\") || p.starts_with("fx/"))
            .unwrap_or(false);
        // Whether this ref would reach the light parse if the
        // no-model filter exempted LIGH bases: not disabled, not
        // a marker, not an fx effect. Today the filter drops
        // every model-less ref, so the real survival count for
        // model-less LIGHs is 0.
        if r.flags & FLAG_INITIALLY_DISABLED == 0 && r.flags & FLAG_IS_MARKER == 0 && !is_effect {
            ligh_would_survive += 1;
        }
        let data_ok = data.map(|d| d.len() >= 12).unwrap_or(false);
        println!(
            "LIGH ref 0x{:08x} base {} (0x{:08x}) pos {:?} radius {} color 0x{:06x} flags 0x{:x} falloff {:.2} fov {:.1} time {} modl={} effect={} data_ok={}",
            r.form_id,
            base.edid().unwrap_or_default(),
            base_id,
            pos,
            radius,
            color,
            flags,
            falloff,
            fov,
            time,
            has_modl,
            is_effect,
            data_ok,
        );
    }
    println!("---");
    println!("LIGH refs in inn: {ligh_refs} (disabled {ligh_disabled}, marker {ligh_marker}, with MODL {ligh_with_modl}, model-less {}, would-survive-if-LIGH-exempted {})", ligh_refs - ligh_with_modl, ligh_would_survive);
    println!("base types: {by_base_type:?}");
    println!("loader today: every model-less LIGH ref is dropped by the no-model filter before the light parse, so 0 real lights reach the scene (M12 log: lights: 1 = central fallback only)");
}
