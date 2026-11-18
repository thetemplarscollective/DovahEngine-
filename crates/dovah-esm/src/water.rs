//! Water data: CELL water levels, worldspace defaults, WATR
//! water types.
//!
//! Format (proven against Dylan's Skyrim.esm — see the
//! `water_probe` / `water_flow_probe` examples in dovah-app):
//!
//! - A CELL's `XCLW` is its water height (f32, Skyrim units);
//!   `XCWT` is its water-type form ID. `XCLW` = FLT_MAX (or
//!   absent) means "the worldspace's default".
//! - The default is the second float of the worldspace's
//!   `WRLD/DNAM` (Tamriel: -14000, the sea level), with
//!   `WRLD/NAM2` as its water type. Defaults of 9,999,999 /
//!   -500,000 mean NO water (worldspace markers —
//!   MossMotherCavernWorld / DeepwoodRedoubtWorld).
//! - `WATR/DNAM` is 228 bytes (232 in SE for 8 records, an
//!   extra trailing Flowmap Scale float; no field moves):
//!   reflectivity f32 @20, fresnel f32 @24, shallow colour
//!   u32 @40, deep colour u32 @44, reflection colour u32 @48.
//!   Colours are little-endian RGB + pad byte.
//! - `WATR/NAM0` is the flow vector (3 f32s, Skyrim space,
//!   z = 0): its signs match the compass in the type's EDID
//!   (RiverWaterFlowNE = (+2.54, +1.35); NW = (-1.96, +2.00);
//!   SE = (+1.80, -2.20)) — proven by `water_flow_probe`,
//!   which diffed the directional types. GNAM/NAM1 are all
//!   zeros on every river type checked, and DNAM @4 (90.0) is
//!   identical across NE/NW/SE, so neither is the direction.
//! - `WATR/NNAM` is the water normal-map texture path
//!   (`Data\Textures\Water\DefaultWater.dds` for the river
//!   types; several identical entries — the first wins).

use crate::Record;

/// `XCLW` = FLT_MAX means "use the worldspace default".
pub const WATER_LEVEL_DEFAULT: f32 = f32::MAX;

/// Worldspace default water levels that mean NO water.
pub const NO_WATER_MARKERS: [f32; 2] = [9_999_999.0, -500_000.0];

/// One parsed WATR (water type) record: the colours, optics,
/// flow, and normal map the renderer needs. All values are
/// read from the record — nothing is tuned by hand except how
/// the renderer uses them.
#[derive(Debug, Clone, PartialEq)]
pub struct WaterType {
    pub form_id: u32,
    /// DNAM @20: reflectivity amount (0.0–1.0).
    pub reflectivity: f32,
    /// DNAM @24: fresnel amount (0.0–1.0).
    pub fresnel: f32,
    /// DNAM @40: shallow-water colour (sRGB bytes).
    pub shallow: [u8; 3],
    /// DNAM @44: deep-water colour (sRGB bytes).
    pub deep: [u8; 3],
    /// DNAM @48: reflection colour (sRGB bytes).
    pub reflection: [u8; 3],
    /// NAM0 (x, y): flow direction × speed, Skyrim space.
    /// [0, 0] when the record has no NAM0 (still water).
    pub flow: [f32; 2],
    /// First NNAM: the water normal-map texture path.
    pub normal_map: Option<String>,
}

/// One cell's resolved water surface: the grid cell, the water
/// height in Skyrim units, and the parsed water type. Built by
/// the app's cell loader (which also checks the terrain dips
/// below the level); the renderer spawns one flat plane per
/// surface.
#[derive(Debug, Clone, PartialEq)]
pub struct WaterSurface {
    pub grid: (i32, i32),
    pub level: f32,
    pub watr: WaterType,
}

fn f32_at(data: &[u8], off: usize) -> Option<f32> {
    let b: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
    Some(f32::from_le_bytes(b))
}

fn u32_at(data: &[u8], off: usize) -> Option<u32> {
    let b: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(b))
}

/// A WATR colour: u32 little-endian RGB + pad byte (low byte
/// first — deep 0x00120e05 is (5, 14, 18), not (18, 14, 5)).
fn rgb_at(data: &[u8], off: usize) -> Option<[u8; 3]> {
    let u = u32_at(data, off)?;
    Some([
        (u & 0xff) as u8,
        ((u >> 8) & 0xff) as u8,
        ((u >> 16) & 0xff) as u8,
    ])
}

/// A worldspace's default water: (level, type form ID) from
/// `WRLD/DNAM` (second float) and `WRLD/NAM2`. Either may be
/// absent (no DNAM / no NAM2).
pub fn parse_wrld_water(wrld: &Record) -> (Option<f32>, Option<u32>) {
    let level = wrld.field(b"DNAM").and_then(|f| f32_at(&f.data, 4));
    let watr = wrld.field(b"NAM2").and_then(|f| u32_at(&f.data, 0));
    (level, watr)
}

/// A cell's own water: (XCLW height, XCWT type form ID).
/// Either may be absent; XCLW may be FLT_MAX (the default
/// sentinel — see [`resolve_water_level`]).
pub fn parse_cell_water(cell: &Record) -> (Option<f32>, Option<u32>) {
    let level = cell.field(b"XCLW").and_then(|f| f32_at(&f.data, 0));
    let watr = cell.field(b"XCWT").and_then(|f| u32_at(&f.data, 0));
    (level, watr)
}

/// Resolve a cell's water level: its own XCLW wins unless it is
/// FLT_MAX (or absent), in which case the worldspace default
/// applies. Returns `None` when there is no level at all, or
/// when the resolved level is a no-water marker (9,999,999 /
/// -500,000 — checked on the resolved level either way: a cell
/// level that high is never real water).
pub fn resolve_water_level(cell_level: Option<f32>, default_level: Option<f32>) -> Option<f32> {
    let level = match cell_level {
        Some(h) if h != WATER_LEVEL_DEFAULT => h,
        _ => default_level?,
    };
    if NO_WATER_MARKERS.contains(&level) {
        return None;
    }
    Some(level)
}

/// Parse a WATR record into a [`WaterType`]. Returns `None`
/// when the record has no DNAM field or the field is shorter
/// than 52 bytes (the last field read is the reflection colour
/// @48..52). The 228- and 232-byte layouts parse identically —
/// the extra SE float sits past every field read here.
pub fn parse_watr(record: &Record) -> Option<WaterType> {
    let dnam = record.field(b"DNAM")?;
    if dnam.data.len() < 52 {
        return None;
    }
    let flow = match record.field(b"NAM0") {
        Some(f) => [
            f32_at(&f.data, 0).unwrap_or(0.0),
            f32_at(&f.data, 4).unwrap_or(0.0),
        ],
        None => [0.0, 0.0],
    };
    Some(WaterType {
        form_id: record.form_id,
        reflectivity: f32_at(&dnam.data, 20).unwrap_or(0.0),
        fresnel: f32_at(&dnam.data, 24).unwrap_or(0.0),
        shallow: rgb_at(&dnam.data, 40).unwrap_or([0, 0, 0]),
        deep: rgb_at(&dnam.data, 44).unwrap_or([0, 0, 0]),
        reflection: rgb_at(&dnam.data, 48).unwrap_or([0, 0, 0]),
        flow,
        normal_map: record.field(b"NNAM").map(|f| f.as_str()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Field;

    fn record(form_id: u32, fields: Vec<(&[u8; 4], Vec<u8>)>) -> Record {
        Record {
            record_type: *b"WATR",
            flags: 0,
            form_id,
            version: 44,
            fields: fields
                .into_iter()
                .map(|(t, data)| Field {
                    field_type: *t,
                    data,
                })
                .collect(),
        }
    }

    fn f32_bytes(v: f32) -> [u8; 4] {
        v.to_le_bytes()
    }

    /// RiverWaterFlowNE's real DNAM values at the documented
    /// offsets (from `water_probe`), in a 228-byte field.
    fn river_dnam() -> Vec<u8> {
        let mut d = vec![0u8; 228];
        d[20..24].copy_from_slice(&f32_bytes(0.8)); // reflectivity
        d[24..28].copy_from_slice(&f32_bytes(0.1)); // fresnel
        d[40..44].copy_from_slice(&0x0018_2726u32.to_le_bytes()); // shallow (38,39,24)
        d[44..48].copy_from_slice(&0x0012_0e05u32.to_le_bytes()); // deep (5,14,18)
        d[48..52].copy_from_slice(&0x009d_8c77u32.to_le_bytes()); // reflection (119,140,157)
        d
    }

    fn nam0(x: f32, y: f32) -> Vec<u8> {
        let mut v = Vec::with_capacity(12);
        v.extend_from_slice(&f32_bytes(x));
        v.extend_from_slice(&f32_bytes(y));
        v.extend_from_slice(&f32_bytes(0.0));
        v
    }

    #[test]
    fn parse_watr_reads_the_documented_offsets() {
        let r = record(
            0x000e717c,
            vec![
                (b"DNAM", river_dnam()),
                (b"NAM0", nam0(2.54, 1.35)),
                (
                    b"NNAM",
                    b"Data\\Textures\\Water\\DefaultWater.dds\0".to_vec(),
                ),
            ],
        );
        let w = parse_watr(&r).expect("parse");
        assert_eq!(w.form_id, 0x000e717c);
        assert!((w.reflectivity - 0.8).abs() < 1e-6);
        assert!((w.fresnel - 0.1).abs() < 1e-6);
        assert_eq!(w.shallow, [38, 39, 24]);
        assert_eq!(w.deep, [5, 14, 18]);
        assert_eq!(w.reflection, [119, 140, 157]);
        assert_eq!(w.flow, [2.54, 1.35]);
        assert_eq!(
            w.normal_map.as_deref(),
            Some("Data\\Textures\\Water\\DefaultWater.dds")
        );
    }

    #[test]
    fn parse_watr_flow_signs_match_the_compass() {
        // The water_flow_probe proof, as unit tests: NW is
        // (-x, +y), SE is (+x, -y). A sign flip here would send
        // the river flowing the wrong way.
        for (x, y, name) in [(2.54, 1.35, "NE"), (-1.96, 2.0, "NW"), (1.8, -2.2, "SE")] {
            let r = record(0x1, vec![(b"DNAM", river_dnam()), (b"NAM0", nam0(x, y))]);
            let w = parse_watr(&r).expect("parse");
            assert_eq!(w.flow, [x, y], "{name}");
        }
    }

    #[test]
    fn parse_watr_accepts_232_byte_dnam_and_rejects_short() {
        let mut d232 = river_dnam();
        d232.extend_from_slice(&f32_bytes(1.5)); // trailing Flowmap Scale
        assert_eq!(d232.len(), 232);
        let r = record(0x1, vec![(b"DNAM", d232)]);
        let w = parse_watr(&r).expect("232-byte DNAM parses");
        assert_eq!(w.deep, [5, 14, 18]);

        let r = record(0x1, vec![(b"DNAM", vec![0u8; 51])]);
        assert!(parse_watr(&r).is_none());
        let r = record(0x1, vec![]);
        assert!(parse_watr(&r).is_none());
    }

    #[test]
    fn parse_watr_without_nam0_is_still() {
        let r = record(0x1, vec![(b"DNAM", river_dnam())]);
        let w = parse_watr(&r).expect("parse");
        assert_eq!(w.flow, [0.0, 0.0]);
        assert_eq!(w.normal_map, None);
    }

    #[test]
    fn resolve_prefers_cell_level_then_default() {
        assert_eq!(
            resolve_water_level(Some(-250.0), Some(-14000.0)),
            Some(-250.0)
        );
        // FLT_MAX sentinel -> default.
        assert_eq!(
            resolve_water_level(Some(f32::MAX), Some(-14000.0)),
            Some(-14000.0)
        );
        // Absent cell level -> default.
        assert_eq!(resolve_water_level(None, Some(-14000.0)), Some(-14000.0));
        // No default either -> no water.
        assert_eq!(resolve_water_level(None, None), None);
        assert_eq!(resolve_water_level(Some(f32::MAX), None), None);
    }

    #[test]
    fn resolve_rejects_no_water_markers() {
        for marker in NO_WATER_MARKERS {
            // As the default (the documented worldspace case).
            assert_eq!(resolve_water_level(None, Some(marker)), None);
            assert_eq!(resolve_water_level(Some(f32::MAX), Some(marker)), None);
            // As a cell level (never real water either).
            assert_eq!(resolve_water_level(Some(marker), Some(-14000.0)), None);
        }
    }

    #[test]
    fn parse_wrld_and_cell_water_read_their_fields() {
        let mut dnam = Vec::new();
        dnam.extend_from_slice(&f32_bytes(-27000.0)); // default land height
        dnam.extend_from_slice(&f32_bytes(-14000.0)); // default water height
        let wrld = record(
            0x3c,
            vec![
                (b"DNAM", dnam),
                (b"NAM2", 0x00000018u32.to_le_bytes().to_vec()),
            ],
        );
        assert_eq!(parse_wrld_water(&wrld), (Some(-14000.0), Some(0x00000018)));

        let cell = record(
            0x9712,
            vec![
                (b"XCLW", f32_bytes(-250.0).to_vec()),
                (b"XCWT", 0x000e717cu32.to_le_bytes().to_vec()),
            ],
        );
        assert_eq!(parse_cell_water(&cell), (Some(-250.0), Some(0x000e717c)));

        let bare = record(0x1, vec![]);
        assert_eq!(parse_wrld_water(&bare), (None, None));
        assert_eq!(parse_cell_water(&bare), (None, None));
    }
}
