//! LAND terrain: decode a Skyrim exterior cell's heightmap and
//! build its terrain mesh.
//!
//! Format (proven against the real Skyrim.esm — see
//! `docs/formats/land.md` and the `vhgt_sweep` / `vhgt_offset`
//! examples, which validated the decode on all 11,186 Tamriel
//! LAND records):
//!
//! - `VHGT` is 1096 bytes: a float32 first height (the southwest
//!   corner), 33x33 int8 deltas, and 3 trailing zero bytes.
//! - Heights accumulate MIXED: the first column accumulates down
//!   (each row's first vertex = the previous row's first vertex +
//!   its delta), and each row accumulates across from its first
//!   vertex. (A plain sequential cumulative sum is wrong — it
//!   makes neighbouring cells' shared edges mismatch by thousands
//!   of units; the mixed accumulation matches every shared edge
//!   vertex exactly.)
//! - The accumulated height — anchor included — is multiplied
//!   by [`HEIGHT_SCALE`] (8) to get game units (M13, settled by
//!   the 1,298-REFR audit in `docs/mod-tools-reference.md` §1).
//! - The LAND `DATA` field (values 25/27/29/31) is NOT a height
//!   offset — adding it breaks edge continuity across Tamriel
//!   (57,049 bad vertices vs 290 without it). Do not add it.
//! - Row 0 is the cell's south edge, column 0 the west edge.
//!   Vertex spacing is 128 Skyrim units; a cell spans 4096.
//! - `VNML` is 33x33x3 int8 normals (row-major, same order).
//! - `VCLR` is 33x33x3 uint8 vertex colors (row-major) — the
//!   terrain's base coloring (grass green, dirt brown, rock grey).

use crate::coords;
use bevy::asset::RenderAssetUsages;
use bevy::math::Vec3;
use bevy::mesh::{Indices, Mesh};
use dovah_esm::Record;

/// VHGT height scale (M13, SETTLED): the accumulated VHGT value —
/// anchor included — is multiplied by 8 to get game units
/// (OpenMW `Land::sHeightScale = 8.0f`; UESP: "The final vertex Z,
/// in game units relative to the cell origin, is the cumulative
/// result multiplied by 8"). Proven directly against Dylan's
/// Skyrim.esm: terrain vs 1,298 Riverwood REFRs gives median error
/// −0.03 m under ×8 vs +0.95 m under ×1, and mountain trees are
/// exact to centimetres under ×8 while floating ~31 m under ×1.
/// A uniform scale error is INVISIBLE to edge-continuity checks,
/// which is why the M9/M11 edge proofs never caught it — and why
/// ×1 terrain sat ~1 m too high in the village (ground up the
/// buildings) while sitting ~30 m too low on the mountain.
/// See `docs/mod-tools-reference.md` §1 and
/// `docs/formats/land.md`.
pub const HEIGHT_SCALE: f32 = 8.0;

/// Vertices per side of a LAND heightmap.
pub const LAND_SIZE: usize = 33;
/// Vertex spacing in Skyrim units.
pub const VERTEX_SPACING: f32 = 128.0;
/// Cell span in Skyrim units (32 intervals of 128).
pub const CELL_SPAN: f32 = 4096.0;

/// One VTXT texture layer on a LAND quadrant (M11 item 5).
/// The terrain's color comes from these layers blended over the
/// BTXT base texture — VCLR is only a near-white tint on top.
#[derive(Debug, Clone)]
pub struct LandTextureLayer {
    /// LTEX form ID of this layer's texture.
    pub ltex: u32,
    /// Quadrant: 0 = SW, 1 = SE, 2 = NW, 3 = NE.
    pub quadrant: u8,
    /// Layer index within the quadrant (0 = lowest, blends first).
    pub layer: u16,
    /// Opacity per quadrant vertex, 17x17 row-major, bottom-left
    /// origin: index `ly * 17 + lx`, where the VTXT position
    /// `p` maps to `(p % 17, p / 17)`. Vertices with no VTXT
    /// entry are 0.0 (the layer does not cover them).
    pub opacity: [f32; QUADRANT_SIZE * QUADRANT_SIZE],
    /// Resolved diffuse DDS path (LTEX -> TXST -> TX00), filled
    /// in by `cell.rs` at load time; `None` when unresolvable.
    pub diffuse: Option<String>,
}

/// Vertices per quadrant edge (17x17 grid per quadrant; the four
/// quadrants share border rows/columns: 17+17-1 = 33).
pub const QUADRANT_SIZE: usize = 17;

/// One decoded LAND record: the terrain of a single exterior cell.
#[derive(Debug, Clone)]
pub struct LandData {
    /// Grid coordinates of the cell (XCLC).
    pub grid: (i32, i32),
    /// Heights in Skyrim units, row-major: index `y * 33 + x`,
    /// row 0 = south edge, column 0 = west edge.
    pub heights: [f32; LAND_SIZE * LAND_SIZE],
    /// Vertex colors (RGB bytes), same order as heights, when the
    /// record has VCLR.
    pub colors: Option<[[u8; 3]; LAND_SIZE * LAND_SIZE]>,
    /// Vertex normals (int8 x/y/z), same order, when VNML exists.
    pub normals: Option<[[i8; 3]; LAND_SIZE * LAND_SIZE]>,
    /// Base texture (BTXT) LTEX form ID, when present.
    pub base_ltex: Option<u32>,
    /// Resolved base diffuse DDS path, filled in by `cell.rs`.
    pub base_diffuse: Option<String>,
    /// VTXT layers, in file order (ascending layer within each
    /// quadrant). Empty for LANDs without texture data.
    pub layers: Vec<LandTextureLayer>,
}

impl LandData {
    /// Bilinear height sample at a point in the cell's local
    /// Skyrim coordinates (`lx` 0..4096 west->east, `ly` 0..4096
    /// south->north). Returns `None` outside the cell.
    pub fn sample_height(&self, lx: f32, ly: f32) -> Option<f32> {
        let fx = lx / VERTEX_SPACING;
        let fy = ly / VERTEX_SPACING;
        if !(0.0..=32.0).contains(&fx) || !(0.0..=32.0).contains(&fy) {
            return None;
        }
        let x0 = (fx.floor() as usize).min(31);
        let y0 = (fy.floor() as usize).min(31);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let h00 = self.heights[y0 * LAND_SIZE + x0];
        let h10 = self.heights[y0 * LAND_SIZE + x0 + 1];
        let h01 = self.heights[(y0 + 1) * LAND_SIZE + x0];
        let h11 = self.heights[(y0 + 1) * LAND_SIZE + x0 + 1];
        Some(
            h00 * (1.0 - tx) * (1.0 - ty)
                + h10 * tx * (1.0 - ty)
                + h01 * (1.0 - tx) * ty
                + h11 * tx * ty,
        )
    }

    /// Minimum and maximum height in the cell (Skyrim units).
    pub fn height_range(&self) -> (f32, f32) {
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for h in self.heights {
            min = min.min(h);
            max = max.max(h);
        }
        (min, max)
    }

    /// Build the terrain mesh in Bevy world space (metres): 33x33
    /// vertices with VCLR colors and VNML normals when present.
    /// Winding is upward-facing (verified by the `terrain_mesh`
    /// test — the first quad's geometric normal has +Y).
    pub fn terrain_mesh(&self) -> Mesh {
        let (gx, gy) = self.grid;
        let origin_x = gx as f32 * CELL_SPAN;
        let origin_y = gy as f32 * CELL_SPAN;
        let mut positions = Vec::with_capacity(LAND_SIZE * LAND_SIZE);
        let mut normals = Vec::with_capacity(LAND_SIZE * LAND_SIZE);
        let mut colors = Vec::with_capacity(LAND_SIZE * LAND_SIZE);
        let mut uvs = Vec::with_capacity(LAND_SIZE * LAND_SIZE);
        for y in 0..LAND_SIZE {
            for x in 0..LAND_SIZE {
                let i = y * LAND_SIZE + x;
                let sky_x = origin_x + x as f32 * VERTEX_SPACING;
                let sky_y = origin_y + y as f32 * VERTEX_SPACING;
                let sky_z = self.heights[i];
                positions.push(coords::to_bevy_position([sky_x, sky_y, sky_z]).to_array());
                // VNML normals are int8 in Skyrim space; convert
                // the direction (no unit scaling) and normalize.
                let n = match &self.normals {
                    Some(normals) => {
                        let raw = normals[i];
                        let d = coords::to_bevy_direction([
                            raw[0] as f32,
                            raw[1] as f32,
                            raw[2] as f32,
                        ]);
                        d.normalize_or_zero()
                    }
                    // No normals in the record: straight up (the
                    // mesh is mostly flat; lighting stays sane).
                    None => Vec3::Y,
                };
                normals.push(n.to_array());
                let c = match &self.colors {
                    Some(colors) => {
                        // M11: VCLR bytes are sRGB; Bevy vertex
                        // colors are linear. Feeding the bytes
                        // straight through made the near-white tint
                        // ~23% too bright (213/255 = 0.84 linear
                        // instead of the correct ~0.68).
                        let rgb = colors[i];
                        [
                            srgb_to_linear(rgb[0] as f32 / 255.0),
                            srgb_to_linear(rgb[1] as f32 / 255.0),
                            srgb_to_linear(rgb[2] as f32 / 255.0),
                            1.0,
                        ]
                    }
                    // No vertex colors: neutral grey-green.
                    None => [0.45, 0.5, 0.4, 1.0],
                };
                colors.push(c);
                // UVs across the cell (0..1 west->east, south->
                // north) for the VTXT texture bake (M11 item 5).
                uvs.push([x as f32 / 32.0, y as f32 / 32.0]);
            }
        }
        let mut indices = Vec::with_capacity(32 * 32 * 6);
        for y in 0..32 {
            for x in 0..32 {
                let a = (y * LAND_SIZE + x) as u32;
                let b = (y * LAND_SIZE + x + 1) as u32;
                let c = ((y + 1) * LAND_SIZE + x) as u32;
                let d = ((y + 1) * LAND_SIZE + x + 1) as u32;
                // (a, b, c) and (b, d, c): upward-facing under the
                // Skyrim->Bevy axis map (see the winding test).
                indices.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
        let mut mesh = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(Indices::U32(indices));
        mesh
    }
}

/// sRGB channel (0.0–1.0) to linear light, exact sRGB EOTF.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Decode ONLY the heights of a LAND record, from its raw VHGT
/// field bytes. The map bake needs heights and nothing else —
/// the full decoder also parses VCLR/VNML/VTXT, which costs
/// 2.3 s across Tamriel's 11,186 LANDs for data the bake throws
/// away (map_bake_probe: full decode 2.95 s vs heights-only
/// 0.62 s, byte-identical heights). `decode_land` calls this,
/// so the accumulation lives in exactly one place. Returns
/// `None` when the field is not the expected 1096 bytes.
pub fn decode_land_heights(vhgt_data: &[u8]) -> Option<[f32; LAND_SIZE * LAND_SIZE]> {
    if vhgt_data.len() != 1096 {
        return None;
    }
    // M13: the accumulated height is ×HEIGHT_SCALE (8) to get
    // game units — anchor included, matching OpenMW's
    // `loadLandRecordData` and UESP's snippet. Without it the
    // terrain is 8× too flat AND offset: ~1 m too high in the
    // village, ~30 m too low on the mountain.
    let first = f32::from_le_bytes(vhgt_data[0..4].try_into().unwrap()) * HEIGHT_SCALE;
    let d = |i: usize| vhgt_data[4 + i] as i8 as f32 * HEIGHT_SCALE;
    let mut heights = [0.0f32; LAND_SIZE * LAND_SIZE];
    // Mixed accumulation (see the module docs): the first column
    // accumulates down, each row accumulates across from its
    // first vertex. D[0] is always 0 in the real data (verified
    // across all 11,186 Tamriel LANDs), so h[0][0] == first.
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

/// Decode a LAND record into [`LandData`]. Returns `None` when the
/// record has no VHGT field or the field is not the expected
/// 1096 bytes.
pub fn decode_land(record: &Record, grid: (i32, i32)) -> Option<LandData> {
    let vhgt = record.field(b"VHGT")?;
    let heights = decode_land_heights(&vhgt.data)?;
    let colors = record.field(b"VCLR").and_then(|f| {
        if f.data.len() != LAND_SIZE * LAND_SIZE * 3 {
            return None;
        }
        let mut colors = [[0u8; 3]; LAND_SIZE * LAND_SIZE];
        for i in 0..LAND_SIZE * LAND_SIZE {
            colors[i] = [f.data[i * 3], f.data[i * 3 + 1], f.data[i * 3 + 2]];
        }
        Some(colors)
    });
    let normals = record.field(b"VNML").and_then(|f| {
        if f.data.len() != LAND_SIZE * LAND_SIZE * 3 {
            return None;
        }
        let mut normals = [[0i8; 3]; LAND_SIZE * LAND_SIZE];
        for i in 0..LAND_SIZE * LAND_SIZE {
            normals[i] = [
                f.data[i * 3] as i8,
                f.data[i * 3 + 1] as i8,
                f.data[i * 3 + 2] as i8,
            ];
        }
        Some(normals)
    });
    // VTXT texture layers (M11 item 5). The fields come in order:
    // BTXT (base texture), then strictly alternating (ATXT, VTXT)
    // pairs — each ATXT header names the texture/quadrant/layer
    // of the VTXT field that follows it. The pairing is positional,
    // so this walks `record.fields` in order instead of using
    // `record.field()` (first match only). Verified across all
    // Riverwood LANDs: N ATXT fields and exactly N VTXT fields.
    // ATXT/BTXT are 8 bytes: LTEX formID (u32), quadrant (u8),
    // unknown byte, layer (u16; 0xFFFF for the BTXT base).
    // VTXT entries are 8 bytes: position (u16, 0..288), unknown
    // u16 (padding — ignored), opacity (f32).
    let mut base_ltex = None;
    let mut layers: Vec<LandTextureLayer> = Vec::new();
    let mut pending: Option<(u32, u8, u16)> = None;
    for field in &record.fields {
        if field.field_type == *b"BTXT" && field.data.len() == 8 {
            base_ltex = Some(u32::from_le_bytes(field.data[0..4].try_into().unwrap()));
        } else if field.field_type == *b"ATXT" && field.data.len() == 8 {
            let ltex = u32::from_le_bytes(field.data[0..4].try_into().unwrap());
            let quadrant = field.data[4];
            let layer = u16::from_le_bytes(field.data[6..8].try_into().unwrap());
            pending = Some((ltex, quadrant, layer));
        } else if field.field_type == *b"VTXT" {
            if let Some((ltex, quadrant, layer)) = pending.take() {
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
                layers.push(LandTextureLayer {
                    ltex,
                    quadrant,
                    layer,
                    opacity,
                    diffuse: None,
                });
            }
            // A VTXT with no pending ATXT is malformed; skip it.
        }
    }
    Some(LandData {
        grid,
        heights,
        colors,
        normals,
        base_ltex,
        base_diffuse: None,
        layers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::render::mesh::VertexAttributeValues;

    /// Build a synthetic VHGT field: float first height + 1089
    /// int8 deltas + 3 trailing bytes.
    fn vhgt_bytes(first: f32, deltas: &[i8; LAND_SIZE * LAND_SIZE]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(1096);
        bytes.extend_from_slice(&first.to_le_bytes());
        for d in deltas {
            bytes.push(*d as u8);
        }
        bytes.extend_from_slice(&[0, 0, 0]);
        bytes
    }

    #[test]
    fn mixed_accumulation_matches_the_proven_layout() {
        // Deltas: row 0 rises +1 per vertex across; row 1 starts
        // +10 above row 0's start and falls -1 per vertex; the
        // rest are zero. A sequential cumsum would smear row 0's
        // total rise (+32) into every later row; the mixed
        // accumulation keeps each row independent of the
        // previous row's across-sum.
        let mut deltas = [0i8; LAND_SIZE * LAND_SIZE];
        for d in &mut deltas[1..LAND_SIZE] {
            *d = 1;
        }
        deltas[LAND_SIZE] = 10;
        for x in 1..LAND_SIZE {
            deltas[LAND_SIZE + x] = -1;
        }
        let vhgt = vhgt_bytes(100.0, &deltas);
        let h = decode_land_heights(&vhgt).expect("1096-byte VHGT");
        // M13: heights are ×HEIGHT_SCALE (8) — row 0 is
        // 800, 808, ..., 1056 in game units.
        assert_eq!(h[0], 800.0);
        assert_eq!(h[1], 808.0);
        assert_eq!(h[32], 1056.0);
        // Row 1 starts at 100 + 10 = 110 (NOT 132 + 10, which is
        // what a sequential cumsum would give), then falls —
        // ×8: 880, 872, ..., 624.
        assert_eq!(h[LAND_SIZE], 880.0);
        assert_eq!(h[LAND_SIZE + 1], 872.0);
        assert_eq!(h[LAND_SIZE + 32], 624.0);
        // Row 2 starts at 110 + 0 = 110, flat — ×8: 880.
        assert_eq!(h[2 * LAND_SIZE], 880.0);
        assert_eq!(h[2 * LAND_SIZE + 32], 880.0);
    }

    #[test]
    fn vhgt_heights_scale_by_eight() {
        // M13 regression: the accumulated VHGT value — anchor
        // included — is ×8 to get game units. A flat cell at
        // anchor 10 decodes to 80.0 everywhere, and a single
        // +1 delta at (1, 0) raises that vertex by 8.
        let deltas = [0i8; LAND_SIZE * LAND_SIZE];
        let vhgt = vhgt_bytes(10.0, &deltas);
        let h = decode_land_heights(&vhgt).expect("1096-byte VHGT");
        assert!(h.iter().all(|v| *v == 80.0));
        let mut deltas = [0i8; LAND_SIZE * LAND_SIZE];
        deltas[1] = 1;
        let vhgt = vhgt_bytes(10.0, &deltas);
        let h = decode_land_heights(&vhgt).expect("1096-byte VHGT");
        assert_eq!(h[0], 80.0);
        assert_eq!(h[1], 88.0);
        assert_eq!(h[2], 88.0);
    }

    #[test]
    fn flat_terrain_decodes_flat() {
        let deltas = [0i8; LAND_SIZE * LAND_SIZE];
        let vhgt = vhgt_bytes(-21.0, &deltas);
        let h = decode_land_heights(&vhgt).expect("1096-byte VHGT");
        // M13: −21.0 × 8 = −168.0 game units.
        assert!(h.iter().all(|v| *v == -168.0));
    }

    #[test]
    fn decode_land_heights_matches_decode_land() {
        // The map bake decodes heights only (see
        // `decode_land_heights`): the heights must equal the
        // full decoder's exactly, even on a record carrying
        // VCLR/VNML — and a short VHGT field rejects.
        use dovah_esm::Field;
        let mut deltas = [0i8; LAND_SIZE * LAND_SIZE];
        deltas[1] = 5;
        deltas[LAND_SIZE] = -3;
        deltas[100] = 12;
        let vhgt = vhgt_bytes(100.0, &deltas);
        let record = Record {
            record_type: *b"LAND",
            flags: 0,
            form_id: 0x1234,
            version: 44,
            fields: vec![
                Field {
                    field_type: *b"VHGT",
                    data: vhgt.clone(),
                },
                Field {
                    field_type: *b"VCLR",
                    data: vec![128u8; LAND_SIZE * LAND_SIZE * 3],
                },
                Field {
                    field_type: *b"VNML",
                    data: vec![0u8; LAND_SIZE * LAND_SIZE * 3],
                },
            ],
        };
        let full = decode_land(&record, (0, 0)).expect("decodes");
        let heights = decode_land_heights(&vhgt).expect("decodes");
        assert_eq!(full.heights, heights);
        assert!(decode_land_heights(&vhgt[..100]).is_none());
        assert!(decode_land_heights(&[]).is_none());
    }

    #[test]
    fn sample_height_interpolates_bilinearly() {
        let mut heights = [0.0f32; LAND_SIZE * LAND_SIZE];
        // A ramp rising east: h = x (in vertex units).
        for y in 0..LAND_SIZE {
            for x in 0..LAND_SIZE {
                heights[y * LAND_SIZE + x] = x as f32;
            }
        }
        let land = LandData {
            grid: (5, -11),
            heights,
            colors: None,
            normals: None,
            base_ltex: None,
            base_diffuse: None,
            layers: Vec::new(),
        };
        // Vertex (0,0): 0. Vertex (32,0): 32.
        assert_eq!(land.sample_height(0.0, 0.0), Some(0.0));
        assert_eq!(land.sample_height(4096.0, 0.0), Some(32.0));
        // Halfway between vertices 0 and 1 east: 0.5.
        assert_eq!(land.sample_height(64.0, 0.0), Some(0.5));
        // Outside the cell: None.
        assert_eq!(land.sample_height(-1.0, 0.0), None);
        assert_eq!(land.sample_height(0.0, 4097.0), None);
    }

    #[test]
    fn terrain_mesh_has_full_grid_and_upward_winding() {
        let heights = [0.0f32; LAND_SIZE * LAND_SIZE];
        let land = LandData {
            grid: (0, 0),
            heights,
            colors: None,
            normals: None,
            base_ltex: None,
            base_diffuse: None,
            layers: Vec::new(),
        };
        let mesh = land.terrain_mesh();
        let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() {
            VertexAttributeValues::Float32x3(p) => p.clone(),
            _ => panic!("positions are Float32x3"),
        };
        assert_eq!(positions.len(), LAND_SIZE * LAND_SIZE);
        // 32x32 quads, 2 triangles each, 3 indices each.
        match mesh.indices().unwrap() {
            Indices::U32(indices) => assert_eq!(indices.len(), 32 * 32 * 6),
            _ => panic!("indices are U32"),
        }
        // The first triangle's geometric normal must point up
        // (Bevy +Y): a downward winding would make the terrain
        // invisible from above (backface-culled) and unusable as
        // a floor.
        let a = Vec3::from(positions[0]);
        let b = Vec3::from(positions[1]);
        let c = Vec3::from(positions[LAND_SIZE]);
        let normal = (b - a).cross(c - a);
        assert!(normal.y > 0.0, "first triangle normal {normal:?}");
    }

    #[test]
    fn terrain_mesh_positions_use_the_cell_origin() {
        // Grid (5,-11): the southwest corner sits at Skyrim
        // (20480, -45056) = Bevy (292.57, 0, 643.66) m.
        let heights = [70.0f32; LAND_SIZE * LAND_SIZE];
        let land = LandData {
            grid: (5, -11),
            heights,
            colors: None,
            normals: None,
            base_ltex: None,
            base_diffuse: None,
            layers: Vec::new(),
        };
        let mesh = land.terrain_mesh();
        let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() {
            VertexAttributeValues::Float32x3(p) => p.clone(),
            _ => panic!("positions are Float32x3"),
        };
        let sw = Vec3::from(positions[0]);
        assert!((sw.x - 20480.0 / 70.0).abs() < 0.01, "{sw:?}");
        assert!((sw.y - 1.0).abs() < 0.01, "{sw:?}");
        assert!((sw.z - 45056.0 / 70.0).abs() < 0.01, "{sw:?}");
    }

    #[test]
    fn decode_land_pairs_atxt_vtxt_in_order() {
        // Synthetic LAND: flat VHGT + BTXT + two (ATXT, VTXT)
        // pairs. The pairing is positional — the second VTXT must
        // attach to the second ATXT, not the first.
        use dovah_esm::Field;
        let deltas = [0i8; LAND_SIZE * LAND_SIZE];
        let vhgt = vhgt_bytes(0.0, &deltas);
        let mut btxt = Vec::new();
        btxt.extend_from_slice(&0x1111u32.to_le_bytes());
        btxt.push(0);
        btxt.push(0);
        btxt.extend_from_slice(&0xFFFFu16.to_le_bytes());
        let atxt = |ltex: u32, quadrant: u8, layer: u16| {
            let mut b = Vec::new();
            b.extend_from_slice(&ltex.to_le_bytes());
            b.push(quadrant);
            b.push(0);
            b.extend_from_slice(&layer.to_le_bytes());
            b
        };
        let vtxt = |entries: &[(u16, f32)]| {
            let mut b = Vec::new();
            for (pos, op) in entries {
                b.extend_from_slice(&pos.to_le_bytes());
                b.extend_from_slice(&0u16.to_le_bytes());
                b.extend_from_slice(&op.to_le_bytes());
            }
            b
        };
        let record = Record {
            record_type: *b"LAND",
            flags: 0,
            form_id: 0x1234,
            version: 44,
            fields: vec![
                Field {
                    field_type: *b"VHGT",
                    data: vhgt,
                },
                Field {
                    field_type: *b"BTXT",
                    data: btxt,
                },
                Field {
                    field_type: *b"ATXT",
                    data: atxt(0x2222, 0, 0),
                },
                Field {
                    field_type: *b"VTXT",
                    data: vtxt(&[(0, 1.0), (1, 0.5)]),
                },
                Field {
                    field_type: *b"ATXT",
                    data: atxt(0x3333, 1, 0),
                },
                Field {
                    field_type: *b"VTXT",
                    data: vtxt(&[(288, 0.25)]),
                },
            ],
        };
        let land = decode_land(&record, (5, -11)).unwrap();
        assert_eq!(land.base_ltex, Some(0x1111));
        assert_eq!(land.layers.len(), 2);
        assert_eq!(land.layers[0].ltex, 0x2222);
        assert_eq!(land.layers[0].quadrant, 0);
        assert_eq!(land.layers[0].layer, 0);
        assert_eq!(land.layers[0].opacity[0], 1.0);
        assert_eq!(land.layers[0].opacity[1], 0.5);
        assert_eq!(land.layers[0].opacity[2], 0.0);
        assert_eq!(land.layers[1].ltex, 0x3333);
        assert_eq!(land.layers[1].quadrant, 1);
        assert_eq!(land.layers[1].opacity[288], 0.25);
        assert_eq!(land.layers[1].opacity[0], 0.0);
    }

    #[test]
    fn srgb_to_linear_known_values() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert_eq!(srgb_to_linear(1.0), 1.0);
        // Riverwood's typical VCLR byte 213: sRGB 0.835 -> linear
        // ~0.665, NOT 0.835 (the old straight-through value).
        let v = srgb_to_linear(213.0 / 255.0);
        assert!((v - 0.665).abs() < 0.01, "{v}");
        assert!(v < 213.0 / 255.0);
    }

    #[test]
    fn terrain_mesh_has_uvs_across_the_cell() {
        let heights = [0.0f32; LAND_SIZE * LAND_SIZE];
        let land = LandData {
            grid: (5, -11),
            heights,
            colors: None,
            normals: None,
            base_ltex: None,
            base_diffuse: None,
            layers: Vec::new(),
        };
        let mesh = land.terrain_mesh();
        let uvs = match mesh.attribute(Mesh::ATTRIBUTE_UV_0).unwrap() {
            VertexAttributeValues::Float32x2(u) => u.clone(),
            _ => panic!("UVs are Float32x2"),
        };
        assert_eq!(uvs.len(), LAND_SIZE * LAND_SIZE);
        assert_eq!(uvs[0], [0.0, 0.0]);
        assert_eq!(uvs[32], [1.0, 0.0]);
        assert_eq!(uvs[32 * LAND_SIZE], [0.0, 1.0]);
        assert_eq!(uvs[LAND_SIZE * LAND_SIZE - 1], [1.0, 1.0]);
    }

    #[test]
    fn vclr_converts_srgb_to_linear() {
        let heights = [0.0f32; LAND_SIZE * LAND_SIZE];
        let land = LandData {
            grid: (5, -11),
            heights,
            colors: Some([[213u8; 3]; LAND_SIZE * LAND_SIZE]),
            normals: None,
            base_ltex: None,
            base_diffuse: None,
            layers: Vec::new(),
        };
        let mesh = land.terrain_mesh();
        let colors = match mesh.attribute(Mesh::ATTRIBUTE_COLOR).unwrap() {
            VertexAttributeValues::Float32x4(c) => c.clone(),
            _ => panic!("colors are Float32x4"),
        };
        // 213 sRGB -> ~0.665 linear, not 0.835.
        assert!((colors[0][0] - 0.665).abs() < 0.01, "{:?}", colors[0]);
        assert_eq!(colors[0][3], 1.0);
    }
}
