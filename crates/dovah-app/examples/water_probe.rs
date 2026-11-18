//! Water data probe: what does Skyrim.esm actually say about
//! Riverwood's water?
//!
//! 1. WRLD Tamriel (0x0000003c): DNAM default heights (land f32 @0,
//!    water f32 @4), NAM2 default water type, NAM3 LOD water type.
//! 2. The 9 Riverwood exterior cells (3x3 around (5,-11)): XCLW
//!    (water height f32) and XCWT (water type form ID) per cell,
//!    plus each LAND's height range so we know which cells'
//!    terrain dips below the water level.
//! 3. Every WATR record: EDID, DNAM length, and the fields at the
//!    Mudcrab-documented offsets (reflectivity @20, fresnel @24,
//!    shallow @40, deep @44, reflection @48). Full DNAM dump as
//!    f32s for the water type Riverwood actually uses, to look
//!    for flow-related fields.
//!
//! Usage: cargo run -p dovah-app --example water_probe
//! (run from the workspace root; reads test-data/)

use dovah_app::cell::TamrielCells;
use dovah_esm::{Plugin, Record};

const ESM_PATH: &str = "test-data/Skyrim.esm";
const TAMRIEL_WRLD: u32 = 0x0000003c;
const CENTER: (i32, i32) = (5, -11);

fn f32_at(data: &[u8], off: usize) -> Option<f32> {
    let b: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
    Some(f32::from_le_bytes(b))
}

fn u32_at(data: &[u8], off: usize) -> Option<u32> {
    let b: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(b))
}

fn rgb(u: u32) -> (u8, u8, u8, u8) {
    (
        (u & 0xff) as u8,
        ((u >> 8) & 0xff) as u8,
        ((u >> 16) & 0xff) as u8,
        ((u >> 24) & 0xff) as u8,
    )
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    let mut by_form_id = std::collections::HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }

    // --- 1. WRLD Tamriel defaults ---
    println!("=== WRLD Tamriel (0x{TAMRIEL_WRLD:08x}) ===");
    let Some(wrld) = by_form_id.get(&TAMRIEL_WRLD) else {
        println!("WRLD Tamriel NOT FOUND");
        return;
    };
    println!("  EDID: {:?}", wrld.edid());
    if let Some(dnam) = wrld.field(b"DNAM") {
        println!("  DNAM len {} bytes", dnam.data.len());
        for i in 0..dnam.data.len() / 4 {
            println!("    f32[{}] = {:?}", i, f32_at(&dnam.data, i * 4));
        }
    } else {
        println!("  NO DNAM field");
    }
    for ft in [b"NAM2", b"NAM3"] {
        match wrld.field(ft) {
            Some(f) => {
                let id = u32_at(&f.data, 0).unwrap_or(0);
                let edid = by_form_id
                    .get(&id)
                    .and_then(|r| r.edid())
                    .unwrap_or_default();
                println!("  {} = 0x{id:08x} ({edid})", String::from_utf8_lossy(ft));
            }
            None => println!("  {} absent", String::from_utf8_lossy(ft)),
        }
    }

    // --- 2. Riverwood 3x3 cells ---
    println!("\n=== Riverwood 3x3 cells (XCLW / XCWT / LAND range) ===");
    let tamriel = TamrielCells::build(&plugin).expect("Tamriel cell map");
    // LAND height ranges need the cell children; reuse load_exterior's
    // LAND decode via the cell index in dovah_app.
    let cell_data =
        dovah_app::cell::load_exterior(&plugin, CENTER, None).expect("load Riverwood exterior");
    let mut ranges = std::collections::HashMap::new();
    for land in &cell_data.lands {
        ranges.insert(land.grid, land.height_range());
    }
    for gy in (CENTER.1 - 1)..=(CENTER.1 + 1) {
        for gx in (CENTER.0 - 1)..=(CENTER.0 + 1) {
            let Some(cell) = tamriel.get((gx, gy)) else {
                println!("  grid ({gx},{gy}): NO CELL");
                continue;
            };
            let xclw = cell.field(b"XCLW").and_then(|f| f32_at(&f.data, 0));
            let xcwt = cell.field(b"XCWT").and_then(|f| u32_at(&f.data, 0));
            let xcwt_edid = xcwt
                .and_then(|id| by_form_id.get(&id))
                .and_then(|r| r.edid())
                .unwrap_or_default();
            let range = ranges.get(&(gx, gy)).copied();
            println!(
                "  grid ({gx},{gy}): CELL {} (0x{:08x}) XCLW={xclw:?} XCWT={} ({xcwt_edid}) LAND range={range:?}",
                cell.edid().unwrap_or_default(),
                cell.form_id,
                xcwt.map(|id| format!("0x{id:08x}")).unwrap_or_default(),
            );
        }
    }

    // --- 3. All WATR records ---
    println!("\n=== WATR records ===");
    let mut watrs: Vec<&Record> = plugin
        .all_records()
        .into_iter()
        .filter(|r| r.type_str() == "WATR")
        .collect();
    watrs.sort_by_key(|r| r.form_id);
    println!("  {} WATR records in Skyrim.esm", watrs.len());
    for w in &watrs {
        let dnam = w.field(b"DNAM");
        let len = dnam.map(|f| f.data.len()).unwrap_or(0);
        let (refl, fres, shallow, deep, refl_col) = match dnam {
            Some(f) => (
                f32_at(&f.data, 20),
                f32_at(&f.data, 24),
                u32_at(&f.data, 40).map(rgb),
                u32_at(&f.data, 44).map(rgb),
                u32_at(&f.data, 48).map(rgb),
            ),
            None => (None, None, None, None, None),
        };
        println!(
            "  0x{:08x} {} DNAM={len}B reflectivity={refl:?} fresnel={fres:?} shallow={shallow:?} deep={deep:?} reflection={refl_col:?}",
            w.form_id,
            w.edid().unwrap_or_default(),
        );
    }

    // --- 4. Full DNAM dump for the water type Riverwood uses ---
    // (the XCWT on the centre cell, else the WRLD NAM2 default)
    let river_watr = tamriel
        .get(CENTER)
        .and_then(|c| c.field(b"XCWT"))
        .and_then(|f| u32_at(&f.data, 0))
        .or_else(|| wrld.field(b"NAM2").and_then(|f| u32_at(&f.data, 0)));
    if let Some(id) = river_watr {
        if let Some(w) = by_form_id.get(&id) {
            println!(
                "\n=== Full DNAM dump: Riverwood water type 0x{id:08x} {} ===",
                w.edid().unwrap_or_default()
            );
            if let Some(dnam) = w.field(b"DNAM") {
                for i in 0..dnam.data.len() / 4 {
                    let off = i * 4;
                    let f = f32_at(&dnam.data, off).unwrap();
                    let u = u32_at(&dnam.data, off).unwrap();
                    println!("  @{off:3} f32={f:>14.6} u32=0x{u:08x} ({u})");
                }
            }
            // Other WATR fields (textures etc.)
            for f in &w.fields {
                if f.field_type != *b"DNAM" {
                    println!(
                        "  field {} len {}: {}",
                        f.type_str(),
                        f.data.len(),
                        if f.data.len() <= 64 {
                            f.as_str()
                        } else {
                            format!("<{} bytes>", f.data.len())
                        }
                    );
                }
            }
        }
    }

    // --- 5. LAND decode sanity: which Riverwood cells dip below
    // the resolved water level? (uses the centre cell's XCLW or
    // the WRLD default)
    let default_water = wrld
        .field(b"DNAM")
        .and_then(|f| f32_at(&f.data, 4))
        .unwrap_or(f32::MAX);
    println!("\n=== Water coverage vs terrain (default water {default_water}) ===");
    for land in &cell_data.lands {
        let (min_h, max_h) = land.height_range();
        let cell = tamriel.get(land.grid);
        let xclw = cell
            .and_then(|c| c.field(b"XCLW"))
            .and_then(|f| f32_at(&f.data, 0));
        let level = match xclw {
            Some(h) if h != f32::MAX => h,
            _ => default_water,
        };
        let below = land.heights.iter().filter(|h| **h < level).count();
        println!(
            "  grid {:?}: terrain {:.0}..{:.0}, water level {:.0} ({}) -> {} of 1089 verts below water{}",
            land.grid,
            min_h,
            max_h,
            level,
            if xclw.is_some_and(|h| h != f32::MAX) {
                "XCLW"
            } else {
                "WRLD default"
            },
            below,
            if below > 0 { "  <-- WATER HERE" } else { "" },
        );
    }
}
