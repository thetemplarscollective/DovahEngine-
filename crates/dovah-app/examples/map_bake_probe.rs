//! Map-bake ANR probe: break down the world-map bake (UI M2's
//! `bake_map_image` + `collect_markers`) into timed stages on
//! Dylan's real Skyrim.esm, so the chunked-spawn fix targets
//! the real cost instead of guessing.
//!
//! Stages (each timed separately):
//!   1. TamrielCells::build — find Tamriel's cell grid.
//!   2. CellIndex::build — index every record's children
//!      (built TWICE by the current code: once in the bake,
//!      once in the marker census).
//!   3. LAND decode loop — the full `decode_land` work per
//!      cell (VHGT + VCLR + VNML + VTXT walk), replicated
//!      faithfully (land.rs is a private module, so the probe
//!      carries its own copy — timing only, never shipped).
//!      The heights-only decode (the VHGT part alone) is
//!      timed separately, to size the win from decoding only
//!      what the bake uses.
//!   4. Sampling loop — 1905x1505 `sample_cell` + HashMap
//!      lookups (the real `sample_cell`).
//!   5. Shading loop — the real `height_shade` over 2.87M px.
//!   6. Marker census — the 6 persistent cells' XMRK scan
//!      (index reuse; string lookups straight from the
//!      Skyrim .STRINGS file — GameStrings lives in the
//!      crate-private dialogue module, so the probe reads
//!      the file the same way `set_for().general()` does).
//!
//! The end-to-end total comes from the real app run
//! (`--character-tab map` logs "map index built: N markers
//! (X ms)"); the stages here must sum to approximately it.
//!
//! Run from the workspace root:
//!   cargo run -p dovah-app --example map_bake_probe

use dovah_app::cell::{CellIndex, TamrielCells};
use dovah_app::character::{height_shade, sample_cell};
use dovah_esm::{Plugin, Record};
use dovah_strings::{StringsFile, StringsKind};
use std::collections::HashMap;
use std::time::Instant;

const ESM_PATH: &str = "test-data/Skyrim.esm";
const STRINGS_DIR: &str = "test-data/strings/Data/Strings";

// Replicated from character.rs (private consts there).
const MAP_BAKE_W: usize = 119 * 16 + 1;
const MAP_BAKE_H: usize = 94 * 16 + 1;
// Replicated from land.rs (private module).
const LAND_SIZE: usize = 33;
const HEIGHT_SCALE: f32 = 8.0;
const QUADRANT_SIZE: usize = 17;

/// Faithful copy of `decode_land`'s per-cell work (land.rs),
/// heights only — the bake's `lands` map holds just the
/// heights, but the real decoder also parses VCLR/VNML/VTXT.
/// This variant times the VHGT accumulation alone.
fn decode_heights_only(record: &Record) -> Option<[f32; LAND_SIZE * LAND_SIZE]> {
    let vhgt = record.field(b"VHGT")?;
    if vhgt.data.len() != 1096 {
        return None;
    }
    let first = f32::from_le_bytes(vhgt.data[0..4].try_into().unwrap()) * HEIGHT_SCALE;
    let d = |i: usize| vhgt.data[4 + i] as i8 as f32 * HEIGHT_SCALE;
    let mut heights = [0.0f32; LAND_SIZE * LAND_SIZE];
    let mut col_acc = first;
    for y in 0..LAND_SIZE {
        col_acc += d(y * LAND_SIZE);
        let mut row_acc = col_acc;
        heights[y * LAND_SIZE] = row_acc;
        for x in 1..LAND_SIZE {
            row_acc += d(y * LAND_SIZE + x);
            heights[y * LAND_SIZE + x] = row_acc;
        }
    }
    Some(heights)
}

/// Faithful copy of `decode_land`'s FULL per-cell work
/// (land.rs): VHGT accumulation + VCLR + VNML copies + the
/// ordered BTXT/ATXT/VTXT walk with its per-layer opacity
/// arrays. Returns the heights (what the bake keeps); the
/// colors/normals/layers are built and dropped, as the real
/// decoder builds them into a LandData the bake then discards
/// except for heights.
fn decode_full(record: &Record) -> Option<[f32; LAND_SIZE * LAND_SIZE]> {
    let heights = decode_heights_only(record)?;
    // VCLR copy.
    if let Some(f) = record.field(b"VCLR") {
        if f.data.len() == LAND_SIZE * LAND_SIZE * 3 {
            let mut colors = [[0u8; 3]; LAND_SIZE * LAND_SIZE];
            for i in 0..LAND_SIZE * LAND_SIZE {
                colors[i] = [f.data[i * 3], f.data[i * 3 + 1], f.data[i * 3 + 2]];
            }
            std::hint::black_box(&colors);
        }
    }
    // VNML copy.
    if let Some(f) = record.field(b"VNML") {
        if f.data.len() == LAND_SIZE * LAND_SIZE * 3 {
            let mut normals = [[0i8; 3]; LAND_SIZE * LAND_SIZE];
            for i in 0..LAND_SIZE * LAND_SIZE {
                normals[i] = [
                    f.data[i * 3] as i8,
                    f.data[i * 3 + 1] as i8,
                    f.data[i * 3 + 2] as i8,
                ];
            }
            std::hint::black_box(&normals);
        }
    }
    // Ordered BTXT/ATXT/VTXT walk.
    let mut pending: Option<(u32, u8, u16)> = None;
    let mut layers = 0usize;
    for field in &record.fields {
        if field.field_type == *b"BTXT" && field.data.len() == 8 {
            std::hint::black_box(u32::from_le_bytes(field.data[0..4].try_into().unwrap()));
        } else if field.field_type == *b"ATXT" && field.data.len() == 8 {
            let ltex = u32::from_le_bytes(field.data[0..4].try_into().unwrap());
            let quadrant = field.data[4];
            let layer = u16::from_le_bytes(field.data[6..8].try_into().unwrap());
            pending = Some((ltex, quadrant, layer));
        } else if field.field_type == *b"VTXT" {
            if let Some((_ltex, _quadrant, _layer)) = pending.take() {
                let mut opacity = [0.0f32; QUADRANT_SIZE * QUADRANT_SIZE];
                let n_entries = field.data.len() / 8;
                for e in 0..n_entries {
                    let entry = &field.data[e * 8..e * 8 + 8];
                    let pos = u16::from_le_bytes(entry[0..2].try_into().unwrap()) as usize;
                    let op = f32::from_le_bytes(entry[4..8].try_into().unwrap());
                    if pos < opacity.len() {
                        opacity[pos] = op;
                    }
                }
                std::hint::black_box(&opacity);
                layers += 1;
            }
        }
    }
    std::hint::black_box(layers);
    Some(heights)
}

fn main() {
    let t0 = Instant::now();
    let bytes = std::fs::read(ESM_PATH).expect("Skyrim.esm");
    let plugin = Plugin::from_bytes(&bytes).expect("parse");
    println!("ESM parse (NOT part of the bake): {:?}", t0.elapsed());

    // Skyrim .STRINGS for the marker-name lookups (every
    // marker REFR is a Skyrim record, form top byte 0).
    let general = StringsFile::from_bytes(
        &std::fs::read(format!("{STRINGS_DIR}/Skyrim_English.STRINGS")).unwrap(),
        StringsKind::General,
    )
    .unwrap();

    // 1. TamrielCells::build.
    let t = Instant::now();
    let tamriel = TamrielCells::build(&plugin);
    println!("1. TamrielCells::build: {:?}", t.elapsed());

    // 2. CellIndex::build.
    let t = Instant::now();
    let index = CellIndex::build(&plugin);
    println!("2. CellIndex::build: {:?}", t.elapsed());

    // 3. LAND decode loop (full decode_land work).
    let t = Instant::now();
    let mut lands: HashMap<(i32, i32), [f32; LAND_SIZE * LAND_SIZE]> = HashMap::new();
    let mut cells = 0usize;
    if let Some(tamriel) = &tamriel {
        for gx in -57..=61 {
            for gy in -43..=50 {
                let Some(cell) = tamriel.get((gx, gy)) else {
                    continue;
                };
                let children = index.children_of(cell.form_id);
                let Some(land) = children.iter().find(|r| r.type_str() == "LAND") else {
                    continue;
                };
                if let Some(decoded) = decode_full(land) {
                    lands.insert((gx, gy), decoded);
                    cells += 1;
                }
            }
        }
    }
    println!(
        "3. LAND decode loop (full, {cells} cells): {:?}",
        t.elapsed()
    );

    // 3b. Heights-only decode loop (same walk, VHGT only).
    let t = Instant::now();
    let mut lands_h: HashMap<(i32, i32), [f32; LAND_SIZE * LAND_SIZE]> = HashMap::new();
    if let Some(tamriel) = &tamriel {
        for gx in -57..=61 {
            for gy in -43..=50 {
                let Some(cell) = tamriel.get((gx, gy)) else {
                    continue;
                };
                let children = index.children_of(cell.form_id);
                let Some(land) = children.iter().find(|r| r.type_str() == "LAND") else {
                    continue;
                };
                if let Some(decoded) = decode_heights_only(land) {
                    lands_h.insert((gx, gy), decoded);
                }
            }
        }
    }
    println!("3b. LAND decode loop (heights-only): {:?}", t.elapsed());
    // Sanity: the heights-only map must equal the full one.
    assert_eq!(lands.len(), lands_h.len());
    for (k, v) in &lands {
        assert_eq!(v, &lands_h[k]);
    }
    println!("    heights-only == full decode ({} cells)", lands.len());

    // 4. Sampling loop (real sample_cell).
    let t = Instant::now();
    let mut heights = vec![0.0f32; MAP_BAKE_W * MAP_BAKE_H];
    let mut min_h = f32::MAX;
    let mut max_h = f32::MIN;
    for py in 0..MAP_BAKE_H {
        for px in 0..MAP_BAKE_W {
            let ((gx, gy), (vx, vy)) = sample_cell(px, py);
            let h = lands
                .get(&(gx, gy))
                .map(|h| h[vy * LAND_SIZE + vx])
                .unwrap_or(0.0);
            heights[py * MAP_BAKE_W + px] = h;
            min_h = min_h.min(h);
            max_h = max_h.max(h);
        }
    }
    println!(
        "4. sampling loop ({} px, h {:.0}..{:.0}): {:?}",
        MAP_BAKE_W * MAP_BAKE_H,
        min_h,
        max_h,
        t.elapsed()
    );

    // 5. Shading loop (real height_shade).
    let t = Instant::now();
    let span = (max_h - min_h).max(1.0);
    let mut data = vec![0u8; MAP_BAKE_W * MAP_BAKE_H * 4];
    for (i, h) in heights.iter().enumerate() {
        let rgba = height_shade((*h - min_h) / span);
        data[i * 4..i * 4 + 4].copy_from_slice(&rgba);
    }
    std::hint::black_box(&data);
    println!("5. shading loop: {:?}", t.elapsed());

    // 6. Marker census (index reuse; real string lookups).
    let t = Instant::now();
    let cells = [
        0x0000_0d74u32,
        0x0001_a270,
        0x0001_6be0,
        0x0001_691f,
        0x0001_6e02,
        0x0003_7ee6,
    ];
    let mut markers = 0usize;
    for cell_id in cells {
        for record in index.children_of(cell_id) {
            if record.type_str() != "REFR" || record.field(b"XMRK").is_none() {
                continue;
            }
            let Some(data) = record.field(b"DATA") else {
                continue;
            };
            if data.data.len() < 12 {
                continue;
            }
            let _x = f32::from_le_bytes(data.data[0..4].try_into().unwrap());
            let _y = f32::from_le_bytes(data.data[4..8].try_into().unwrap());
            let _name = record
                .field(b"FULL")
                .and_then(|f| f.data.get(0..4))
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .and_then(|i| general.get(i))
                .unwrap_or("");
            markers += 1;
        }
    }
    println!(
        "6. marker census ({markers} markers, index reused): {:?}",
        t.elapsed()
    );
}
