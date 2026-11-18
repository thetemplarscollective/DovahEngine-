//! Cell loading: turn CELL records from a plugin into the list of
//! objects the renderer needs to draw, and (M9) load a block of
//! exterior cells around a village.
//!
//! Pure ESM logic — no Bevy types here. Positions and rotations stay
//! in Skyrim space (units, radians); see [`crate::coords`] for the
//! conversion to Bevy space. Follows the `esm_cell` example's pattern
//! (cell -> child groups -> REFR/ACHR -> base record -> MODL).
//!
//! Field layouts (UESP "Skyrim Mod:Mod File Format", corrected
//! against the real Skyrim.esm where the docs are wrong):
//! - REFR/ACHR `DATA`: 24 bytes, float[3] position + float[3]
//!   rotation (radians).
//! - REFR `XSCL`: one float, uniform scale (default 1.0).
//! - Base `OBND`: 12 bytes, int16 X1/Y1/Z1/X2/Y2/Z2 — the object's
//!   bounding box in its local space.
//! - LIGH `DATA`: 48-byte struct; radius is the uint32 at offset 4,
//!   color is the 0x00BBGGRR uint32 at offset 8, flags is the uint32
//!   at offset 12 (bit 3 = flicker), falloff exponent is the f32 at
//!   offset 16. LIGH bases carry no MODL — they are pure lights,
//!   so the loader exempts them from the no-model filter in
//!   interiors (M14), or the inn loads with zero real lights.
//! - CELL `XCLC`: 12 bytes, NOT 8: int32 grid X, int32 grid Y,
//!   int32 unknown (0x0053fd00 for most Tamriel cells, 0x1700 for
//!   the persistent cell). All 16,978 exterior CELLs carry one.
//! - REFR `XTEL`: 32 bytes, NOT 28: uint32 dest door ref, float[3]
//!   arrival position, float[3] arrival rotation, uint32 tail (0
//!   in every door checked so far).
//! - Persistent refs (teleport doors, quest markers, ...) are filed
//!   under Tamriel's persistent cell (form 0x00000d74, grid (0,0)),
//!   NOT under the cell where they stand. An exterior loader must
//!   sweep the persistent cell by position, or the inn door itself
//!   (and ~139 other Riverwood refs) goes missing.

use crate::land::{decode_land, LandData, CELL_SPAN};
use crate::npc::{npc_is_creature, resolve_npc, NpcIndex};
use bevy::ecs::prelude::Resource;
use dovah_esm::water::{
    parse_cell_water, parse_watr, parse_wrld_water, resolve_water_level, WaterSurface,
};
use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::HashMap;

/// REFR flag: initially disabled — the object is not in the world
/// until something enables it, so the renderer skips it.
const FLAG_INITIALLY_DISABLED: u32 = 0x0000_0800;
/// REFR flag: is a marker (invisible in-game).
const FLAG_IS_MARKER: u32 = 0x0080_0000;
/// Tamriel's worldspace form ID.
pub(crate) const TAMRIEL_WRLD: u32 = 0x0000003c;
/// Tamriel's persistent cell form ID (grid (0,0)). Fixed Bethesda
/// data in every copy of Skyrim.esm — the exterior loader uses it
/// directly. It must NOT be found through the grid map: the
/// geographic cell BleakwindBasinExterior01 (0x000099a2) shares
/// grid (0,0), and whichever CELL the map visits last wins the
/// key. M9 shipped with the grid lookup and loaded the exterior
/// with 0 doors (the persistent refs, inn door included, never
/// arrived) — see `persist_check.rs`.
pub(crate) const PERSISTENT_CELL_FORM_ID: u32 = 0x00000d74;

/// A base object's bounding box in its local space, Skyrim units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub min: [i16; 3],
    pub max: [i16; 3],
}

/// LIGH DATA flag: the light flickers (candles, fires).
const LIGH_FLAG_FLICKER: u32 = 0x0000_0008;

/// LIGH DATA type flag: "Shadow Omnidirectional" (UESP — at
/// most one type flag is set; none set = plain omnidirectional
/// light, no shadows). The other two types, Shadow Spotlight
/// (0x0400) and Shadow Hemisphere (0x0800), are NOT point
/// lights: a spotlight needs a SpotLight, a hemisphere is not a
/// point light at all — both are future work, so only the omni
/// flag is a constant here. Proven by the M16 sweep
/// (`ligh_flag_sweep.rs`): all 37 "Shadow"-named LIGH bases in
/// Skyrim.esm carry exactly one of the three type flags.
const LIGH_FLAG_SHADOW_OMNI: u32 = 0x0000_1000;

/// Light data from a LIGH base record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightData {
    /// Light radius in Skyrim units.
    pub radius: u32,
    /// Color as RGB bytes.
    pub color: [u8; 3],
    /// LIGH DATA flags (bit 0 = dynamic, bit 3 = flicker,
    /// bit 12 = shadow-casting omnidirectional — see
    /// `casts_shadows`). The flicker bit is recorded for the
    /// flicker pass; M14 lights are static.
    #[allow(dead_code)]
    pub flags: u32,
    /// Falloff exponent from LIGH DATA (1.0 for the inn's
    /// lights). Per the Creation Kit wiki this field only
    /// applies to spotlights — it does NOT shape point-light
    /// falloff. Recorded, not used.
    #[allow(dead_code)]
    pub falloff: f32,
    /// Fade from the LIGH record's separate FNAM field — the
    /// light's intensity scale (0.1–10, typically 1.0). This
    /// is the F in Skyrim's linear falloff: attenuation(d) =
    /// Fade x max(0, 1 - d/Radius). Default 1.0 when the record
    /// carries no FNAM field. All three inn light bases have
    /// Fade 1.0 (proven by ligh_data_dump) — the fireplace is
    /// NOT brighter than a sconce in the game's data, so the
    /// old x2 fireplace hack is gone.
    pub fade: f32,
}

impl LightData {
    /// True when the light flickers (candles, fires).
    #[allow(dead_code)]
    pub fn flickers(&self) -> bool {
        self.flags & LIGH_FLAG_FLICKER != 0
    }

    /// True when the light casts shadows as an omnidirectional
    /// (point) light: LIGH DATA type flag 0x1000. The inn's
    /// fireplace and its FarmInteriorShadowLight01 both carry
    /// it; the 6 sconces (0x9) do not. Spotlight (0x0400) and
    /// hemisphere (0x0800) shadow lights return false here —
    /// they are not point lights and need their own
    /// implementation before they can cast shadows.
    pub fn casts_shadows(&self) -> bool {
        self.flags & LIGH_FLAG_SHADOW_OMNI != 0
    }
}

/// A teleport target from a door REFR's XTEL field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XtelTarget {
    /// Form ID of the destination door reference.
    pub dest_ref: u32,
    /// Arrival position in Skyrim space (units).
    pub pos: [f32; 3],
    /// Arrival rotation in Skyrim space (radians).
    pub rot: [f32; 3],
}

/// Where the player arrives when a scene loads: a Skyrim-space
/// position plus the yaw (rotation about Skyrim Z, radians) the
/// player faces. Comes from an XTEL when the load was triggered
/// by walking through a door.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnPoint {
    pub position: [f32; 3],
    pub yaw: f32,
}

impl From<XtelTarget> for SpawnPoint {
    fn from(xtel: XtelTarget) -> Self {
        SpawnPoint {
            position: xtel.pos,
            yaw: xtel.rot[2],
        }
    }
}

/// A scene the app can load: an interior cell by EDID, or a block
/// of exterior cells centred on a Tamriel grid cell.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SceneTarget {
    // M4: the form ID rides along with the EDID — the Ruffle
    // HUD's crosshair resolves a load door's destination name
    // from the destination CELL's FULL (via `plugin.find`), and
    // there is no EDID lookup. `form_id` is 0 when unknown (the
    // `--cell` CLI target, resolved by EDID at load time).
    Interior { form_id: u32, edid: String },
    Exterior { grid: (i32, i32) },
}

impl SceneTarget {
    /// Display name for the loading screen.
    pub fn display_name(&self) -> String {
        match self {
            SceneTarget::Interior { edid, .. } => edid.clone(),
            SceneTarget::Exterior { grid } => {
                format!("Riverwood ({}, {})", grid.0, grid.1)
            }
        }
    }
}

/// A door the player can walk through: its trigger position (the
/// door ref's own position — where the player walks) and where it
/// leads (resolved at load time from the XTEL).
#[derive(Debug, Clone, PartialEq)]
pub struct DoorTrigger {
    pub ref_form_id: u32,
    /// Door position in Skyrim space (units).
    pub trigger_pos: [f32; 3],
    pub target: SceneTarget,
    /// Where the player arrives on the other side.
    pub spawn: SpawnPoint,
}

/// One placed object in a cell.
///
/// The form IDs and EDID are kept for diagnostics and for M7
/// (gameplay needs to identify what the player is looking at), even
/// though the M6 renderer only reads the model path and transform.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PlacedObject {
    /// Form ID of the reference itself.
    pub ref_form_id: u32,
    /// Form ID of the base object.
    pub base_form_id: u32,
    /// Base record type, e.g. `STAT`, `FURN`, `NPC_`.
    pub base_type: String,
    /// Base editor ID, e.g. `FarmTableBench01`.
    pub base_edid: String,
    /// Model path from the base record's MODL, e.g.
    /// `Furniture\Farm\FarmTableBench01.nif`. `None` for objects
    /// without a model (triggers, leveled lists, markers).
    pub model_path: Option<String>,
    /// Position in Skyrim space (units).
    pub position: [f32; 3],
    /// Rotation in Skyrim space (radians).
    pub rotation: [f32; 3],
    /// Uniform scale from XSCL (1.0 when absent).
    pub scale: f32,
    /// Base object bounds, when the base record has OBND.
    pub bounds: Option<Bounds>,
    /// Light data, when the base record is a LIGH.
    pub light: Option<LightData>,
    /// True for ACHR (actors) — they get real NPC models from
    /// `actor_models` (M12), or a capsule placeholder when the
    /// resolution fails.
    pub is_actor: bool,
    /// True when the actor is a creature (its NPC_ base's race
    /// lacks the FaceGen-head flag — chicken, cow, horse,
    /// dragon...). Skyrim has no CREA record type; creatures
    /// ARE NPC_ records, so the record type cannot tell them
    /// apart. Creatures get no human body (`resolve_npc`
    /// returns None for them) and are not talkable unless they
    /// have dialogue of their own (Paarthurnax talks; a chicken
    /// does not) — see `dialogue::build_talkable_npcs`.
    pub is_creature: bool,
    /// For actors: the model paths that draw the person, resolved
    /// from the NPC_ record (body parts, outfit armor, FaceGen
    /// head — see [`crate::npc`]). Empty for non-actors, and for
    /// actors whose NPC_ record could not be resolved.
    pub actor_models: Vec<String>,
    /// Teleport target, when the ref is a door with XTEL.
    pub xtel: Option<XtelTarget>,
}

/// A loaded scene, ready for the renderer: one interior cell, or a
/// block of exterior cells (M9) with their terrain.
#[derive(Debug, Resource)]
pub struct CellData {
    pub edid: String,
    pub form_id: u32,
    pub interior: bool,
    /// Objects to draw (markers, triggers, and disabled refs are
    /// already filtered out).
    pub objects: Vec<PlacedObject>,
    /// Terrain for exterior scenes (empty for interiors).
    pub lands: Vec<LandData>,
    /// Water surfaces for exterior scenes (empty for
    /// interiors): one per cell whose terrain dips below its
    /// resolved water level. See [`resolve_waters`].
    pub waters: Vec<WaterSurface>,
    /// Where the player spawns (`None` = the default viewpoint:
    /// the interior camera position, or the exterior door view).
    pub spawn: Option<SpawnPoint>,
    /// Doors the player can walk through (resolved targets).
    pub doors: Vec<DoorTrigger>,
    /// Counts of things skipped, for the scene summary.
    pub skipped_markers: usize,
    pub skipped_disabled: usize,
    pub skipped_no_data: usize,
    pub skipped_no_base: usize,
    pub skipped_no_model: usize,
    pub skipped_other_types: usize,
    /// Effect volumes (meshes\fx\...) — dust, light beams, glows.
    /// Not solid geometry; their OBND is the effect's volume.
    pub skipped_effects: usize,
}

impl CellData {
    /// True when this scene is an exterior block (has terrain).
    pub fn is_exterior(&self) -> bool {
        !self.lands.is_empty()
    }
}

/// Parse a REFR/ACHR DATA field into (position, rotation).
pub fn parse_placement(data: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    if data.len() != 24 {
        return None;
    }
    let f = |i: usize| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    Some(([f(0), f(1), f(2)], [f(3), f(4), f(5)]))
}

/// Parse an OBND field into bounds. Layout: int16 X1, Y1, Z1, X2,
/// Y2, Z2.
pub fn parse_obnd(data: &[u8]) -> Option<Bounds> {
    if data.len() != 12 {
        return None;
    }
    let v = |i: usize| i16::from_le_bytes(data[i * 2..i * 2 + 2].try_into().unwrap());
    Some(Bounds {
        min: [v(0), v(1), v(2)],
        max: [v(3), v(4), v(5)],
    })
}

/// Parse a LIGH DATA field into light data. Radius is the uint32 at
/// offset 4; color is 0x00BBGGRR at offset 8; flags is the uint32
/// at offset 12; falloff exponent is the f32 at offset 16. (SE
/// LIGH DATA is 48 bytes; only the first 20 are read here.)
/// `fade` is the record's separate FNAM field (the light's
/// intensity scale) — the caller reads it from the base record
/// because DATA does not carry it; pass 1.0 when FNAM is absent.
pub fn parse_light(data: &[u8], fade: f32) -> Option<LightData> {
    if data.len() < 20 {
        return None;
    }
    let radius = u32::from_le_bytes(data[4..8].try_into().unwrap());
    let color = u32::from_le_bytes(data[8..12].try_into().unwrap());
    let flags = u32::from_le_bytes(data[12..16].try_into().unwrap());
    let falloff = f32::from_le_bytes(data[16..20].try_into().unwrap());
    Some(LightData {
        radius,
        color: [
            (color & 0xff) as u8,
            ((color >> 8) & 0xff) as u8,
            ((color >> 16) & 0xff) as u8,
        ],
        flags,
        falloff,
        fade,
    })
}

/// Parse an XTEL field into its teleport target. Skyrim SE XTEL is
/// 32 bytes: dest door ref (u32), arrival pos (3x f32), arrival
/// rot (3x f32), trailing u32 (0 in every door checked).
pub fn parse_xtel(data: &[u8]) -> Option<XtelTarget> {
    if data.len() != 32 {
        return None;
    }
    let u = |i: usize| u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    let f = |i: usize| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    Some(XtelTarget {
        dest_ref: u(0),
        pos: [f(1), f(2), f(3)],
        rot: [f(4), f(5), f(6)],
    })
}

/// Parse a CELL XCLC field into grid coordinates. Skyrim SE XCLC
/// is 12 bytes: int32 grid X, int32 grid Y, int32 unknown.
pub fn parse_xclc(data: &[u8]) -> Option<(i32, i32)> {
    if data.len() != 12 {
        return None;
    }
    let gx = i32::from_le_bytes(data[0..4].try_into().unwrap());
    let gy = i32::from_le_bytes(data[4..8].try_into().unwrap());
    Some((gx, gy))
}

/// Grid cell containing a Skyrim-space position.
pub fn grid_of_position(pos: [f32; 3]) -> (i32, i32) {
    (
        (pos[0] / CELL_SPAN).floor() as i32,
        (pos[1] / CELL_SPAN).floor() as i32,
    )
}

fn form_id_from_field(record: &Record, field_type: &[u8; 4]) -> Option<u32> {
    let field = record.field(field_type)?;
    if field.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(field.data[0..4].try_into().unwrap()))
}

/// Resolve an LTEX form ID to its diffuse DDS path (M11 item 5):
/// LTEX `TNAM` -> TXST form ID, TXST `TX00` -> diffuse texture
/// path (e.g. `Landscape\Dirt02.dds`). Returns `None` when any
/// link is missing — the bake then skips that layer.
fn resolve_ltex_diffuse(ltex_id: u32, by_form_id: &HashMap<u32, &Record>) -> Option<String> {
    let ltex = by_form_id.get(&ltex_id)?;
    let txst_id = form_id_from_field(ltex, b"TNAM")?;
    let txst = by_form_id.get(&txst_id)?;
    let path = txst.field(b"TX00")?.as_str();
    if path.is_empty() {
        return None;
    }
    // TX00 paths are relative to the textures folder
    // (`Landscape\Dirt02.dds`); the archive index holds the full
    // `textures\...` path, so prepend it unless already present.
    let lower = path.to_lowercase();
    if lower.starts_with("textures\\") || lower.starts_with("textures/") {
        Some(path)
    } else {
        Some(format!("textures\\{path}"))
    }
}

/// Fill in the resolved diffuse paths on a decoded LAND's base
/// texture and VTXT layers.
fn resolve_land_textures(land: &mut LandData, by_form_id: &HashMap<u32, &Record>) {
    if let Some(ltex) = land.base_ltex {
        land.base_diffuse = resolve_ltex_diffuse(ltex, by_form_id);
    }
    for layer in &mut land.layers {
        layer.diffuse = resolve_ltex_diffuse(layer.ltex, by_form_id);
    }
}

/// Whether a CELL record is an interior (DATA first byte bit 0).
fn cell_is_interior(cell: &Record) -> bool {
    cell.field(b"DATA")
        .and_then(|f| f.data.first())
        .map(|b| b & 0x01 != 0)
        .unwrap_or(false)
}

/// One-pass index over the plugin's group tree: cell form ID ->
/// its child records, and ref form ID -> its parent cell form ID.
///
/// The per-cell walk (`cell_child_records`) is O(groups) per cell
/// — fine for one interior cell, hopeless for 16,978 exterior
/// cells (the first riverwood probe ran past 120 s that way). The
/// index walks the tree once and answers both questions in O(1).
pub struct CellIndex<'a> {
    children: HashMap<u32, Vec<&'a Record>>,
    parent_of_ref: HashMap<u32, u32>,
}

impl<'a> CellIndex<'a> {
    pub fn build(plugin: &'a Plugin) -> Self {
        let mut index = CellIndex {
            children: HashMap::new(),
            parent_of_ref: HashMap::new(),
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
                    self.parent_of_ref.insert(r.form_id, cell_id);
                }
            }
        }
        for subgroup in group.subgroups() {
            self.scan(subgroup);
        }
    }

    pub fn children_of(&self, cell_id: u32) -> &[&'a Record] {
        self.children
            .get(&cell_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn parent_cell_of(&self, ref_form_id: u32) -> Option<u32> {
        self.parent_of_ref.get(&ref_form_id).copied()
    }
}

/// Tamriel's exterior cells by grid coordinates, from the Tamriel
/// WorldChildren group (label = WRLD form 0x0000003c). XCLC grid
/// numbers alone are ambiguous across worldspaces (Sovngarde and
/// other worldspaces reuse the same grid), so the map is built
/// from Tamriel's group only.
pub struct TamrielCells<'a> {
    by_grid: HashMap<(i32, i32), &'a Record>,
}

impl<'a> TamrielCells<'a> {
    pub fn build(plugin: &'a Plugin) -> Option<Self> {
        // Find the Tamriel WorldChildren group.
        let mut stack: Vec<&Group> = plugin.groups.iter().collect();
        let mut tamriel_group: Option<&Group> = None;
        while let Some(g) = stack.pop() {
            if g.group_type == GroupType::WorldChildren && g.label_form_id() == Some(TAMRIEL_WRLD) {
                tamriel_group = Some(g);
                break;
            }
            stack.extend(g.subgroups());
        }
        let tg = tamriel_group?;
        let mut by_grid = HashMap::new();
        let mut stack2: Vec<&Group> = vec![tg];
        while let Some(g) = stack2.pop() {
            for r in g.records() {
                if r.type_str() == "CELL" {
                    // The persistent cell is not a geographic cell:
                    // it shares grid (0,0) with
                    // BleakwindBasinExterior01 and would collide
                    // in this map. It is reached by form ID, not
                    // by grid (see PERSISTENT_CELL_FORM_ID).
                    if r.form_id == PERSISTENT_CELL_FORM_ID {
                        continue;
                    }
                    if let Some(f) = r.field(b"XCLC") {
                        if let Some(grid) = parse_xclc(&f.data) {
                            by_grid.insert(grid, r);
                        }
                    }
                }
            }
            stack2.extend(g.subgroups());
        }
        Some(TamrielCells { by_grid })
    }

    pub fn get(&self, grid: (i32, i32)) -> Option<&'a Record> {
        self.by_grid.get(&grid).copied()
    }
}

/// Find a CELL record by editor ID (exact match, then a
/// case-insensitive fallback).
pub(crate) fn find_cell<'a>(plugin: &'a Plugin, edid: &str) -> Option<&'a Record> {
    plugin
        .all_records()
        .into_iter()
        .find(|r| r.type_str() == "CELL" && r.edid().as_deref() == Some(edid))
        .or_else(|| {
            plugin.all_records().into_iter().find(|r| {
                r.type_str() == "CELL"
                    && r.edid()
                        .map(|e| e.eq_ignore_ascii_case(edid))
                        .unwrap_or(false)
            })
        })
}

/// Whether a base record is an invisible marker. Markers have model
/// paths (e.g. `MarkerX.nif`) but must not be drawn: they are
/// placement aids, not visible objects.
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

/// Whether a ref is enabled at game start: its own
/// initially-disabled flag, plus its XESP enable-parent chain.
/// A ref with an enable parent spawns only when the parent is
/// enabled (XOR the XESP "opposite of parent" flag, bit 0 of
/// byte 4). The parent is resolved through `by_form_id`, which
/// holds every record — enable markers live in the persistent
/// cell with no position and never enter the load set, so they
/// cannot be found among the loaded refs. Chains are walked
/// with a depth cap (real chains are 1-2 deep; the cap also
/// bounds cycles). A missing parent (a DLC record we do not
/// load) counts as enabled — the child keeps the current
/// behavior instead of vanishing.
///
/// The case that found it (v0.4.4 phone frames): the three
/// Stormcloak Riverwood guards (0x0001b74f/50/51) have no
/// outfit and a disabled enable parent
/// (`CWGarrisonEnableMarkerSonsRiverwood`, 0x0001b753, flags
/// 0xC00). The loader ignored XESP, spawned all three, and one
/// of them wandered Riverwood naked. The Imperial guards'
/// parent (0x0001b754) is enabled, so they still spawn.
fn ref_enabled(record: &Record, by_form_id: &HashMap<u32, &Record>, depth: u8) -> bool {
    if record.flags & FLAG_INITIALLY_DISABLED != 0 {
        return false;
    }
    let Some(xesp) = record.field(b"XESP") else {
        return true;
    };
    if xesp.data.len() < 5 || depth >= 8 {
        return true;
    }
    let parent_id = u32::from_le_bytes(xesp.data[0..4].try_into().unwrap());
    let opposite = xesp.data[4] & 1 != 0;
    let Some(parent) = by_form_id.get(&parent_id) else {
        return true;
    };
    ref_enabled(parent, by_form_id, depth + 1) != opposite
}

/// Turn placed records into [`PlacedObject`]s, appending to `data`
/// and updating its skip counts. Shared by the interior and
/// exterior loaders — the filtering rules are the same in both.
fn append_objects(
    records: &[&Record],
    by_form_id: &HashMap<u32, &Record>,
    npc_index: &NpcIndex,
    data: &mut CellData,
) {
    for r in records {
        let kind = r.type_str();
        if kind != "REFR" && kind != "ACHR" {
            data.skipped_other_types += 1;
            continue;
        }
        if !ref_enabled(r, by_form_id, 0) {
            data.skipped_disabled += 1;
            continue;
        }
        if r.flags & FLAG_IS_MARKER != 0 {
            data.skipped_markers += 1;
            continue;
        }
        let Some((position, rotation)) = r.field(b"DATA").and_then(|f| parse_placement(&f.data))
        else {
            data.skipped_no_data += 1;
            continue;
        };
        let Some(base_id) = form_id_from_field(r, b"NAME") else {
            data.skipped_no_base += 1;
            continue;
        };
        let Some(base) = by_form_id.get(&base_id) else {
            data.skipped_no_base += 1;
            continue;
        };
        if is_marker_base(base) {
            data.skipped_markers += 1;
            continue;
        }
        let is_actor = kind == "ACHR";
        let model_path = base.field(b"MODL").map(|f| f.as_str());
        // M14: interior LIGH bases have no model — they are pure
        // light sources. The inn's 8 candles/fire lights are ALL
        // model-less, so the no-model filter used to drop every
        // one of them before the light parse below, and the inn
        // shipped with zero real lights (M12 log: lights: 1 =
        // the central fallback only). Exempt them — but only in
        // interiors: M14 is an interior-only pass and the
        // exterior must stay exactly as M11/M13 left it.
        let is_interior_light = data.interior && base.type_str() == "LIGH";
        if model_path.is_none() && !is_actor && !is_interior_light {
            data.skipped_no_model += 1;
            continue;
        }
        // Effect volumes (dust motes, light beams, magic glows) are
        // not solid geometry — their OBND is the effect's volume,
        // and drawing it as a solid box would fill the room with
        // phantom blocks. Skipped until the real effect pipeline
        // exists (M7+).
        if let Some(path) = &model_path {
            let lower = path.to_ascii_lowercase();
            if lower.starts_with("fx\\") || lower.starts_with("fx/") {
                data.skipped_effects += 1;
                continue;
            }
        }
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
        let bounds = base.field(b"OBND").and_then(|f| parse_obnd(&f.data));
        let light = if base.type_str() == "LIGH" {
            // Fade lives in the record's separate FNAM field
            // (f32, 0.1–10); absent FNAM means 1.0.
            let fade = base
                .field(b"FNAM")
                .filter(|f| f.data.len() == 4)
                .map(|f| f32::from_le_bytes(f.data[0..4].try_into().unwrap()))
                .unwrap_or(1.0);
            base.field(b"DATA").and_then(|f| parse_light(&f.data, fade))
        } else {
            None
        };
        let xtel = r.field(b"XTEL").and_then(|f| parse_xtel(&f.data));
        // Actors: resolve the NPC_ record to its model parts
        // (body, outfit, FaceGen head). An unresolvable NPC keeps
        // an empty list and renders as a capsule in the scene.
        // Creatures resolve to nothing (see `npc_is_creature`) —
        // they are animals, not people in animal suits.
        let actor_models = if is_actor {
            resolve_npc(by_form_id, npc_index, base_id)
                .map(|m| m.parts)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let is_creature = is_actor && npc_is_creature(by_form_id, base_id);
        data.objects.push(PlacedObject {
            ref_form_id: r.form_id,
            base_form_id: base_id,
            base_type: base.type_str(),
            base_edid: base.edid().unwrap_or_default(),
            model_path,
            position,
            rotation,
            scale,
            bounds,
            light,
            is_actor,
            is_creature,
            actor_models,
            xtel,
        });
    }
}

/// Resolve an XTEL into the scene it leads to. The destination
/// door ref's parent cell decides: an interior parent cell means
/// an interior target (by that cell's EDID); anything else
/// (exterior cell, or the persistent cell that holds all of
/// Tamriel's persistent refs) means an exterior target, centred
/// on the grid cell the XTEL position falls in.
fn resolve_xtel_target(
    index: &CellIndex,
    by_form_id: &HashMap<u32, &Record>,
    xtel: &XtelTarget,
) -> SceneTarget {
    if let Some(parent_id) = index.parent_cell_of(xtel.dest_ref) {
        if let Some(cell) = by_form_id.get(&parent_id) {
            if cell.type_str() == "CELL" && cell_is_interior(cell) {
                if let Some(edid) = cell.edid() {
                    return SceneTarget::Interior {
                        form_id: parent_id,
                        edid,
                    };
                }
            }
        }
    }
    SceneTarget::Exterior {
        grid: grid_of_position(xtel.pos),
    }
}

/// Build the door triggers for a loaded scene from its objects'
/// XTEL fields.
fn build_doors(data: &mut CellData, index: &CellIndex, by_form_id: &HashMap<u32, &Record>) {
    let mut doors = Vec::new();
    for o in &data.objects {
        if let Some(xtel) = o.xtel {
            doors.push(DoorTrigger {
                ref_form_id: o.ref_form_id,
                trigger_pos: o.position,
                target: resolve_xtel_target(index, by_form_id, &xtel),
                spawn: SpawnPoint::from(xtel),
            });
        }
    }
    data.doors = doors;
}

fn empty_cell_data(
    edid: String,
    form_id: u32,
    interior: bool,
    spawn: Option<SpawnPoint>,
) -> CellData {
    CellData {
        edid,
        form_id,
        interior,
        objects: Vec::new(),
        lands: Vec::new(),
        waters: Vec::new(),
        spawn,
        doors: Vec::new(),
        skipped_markers: 0,
        skipped_disabled: 0,
        skipped_no_data: 0,
        skipped_no_base: 0,
        skipped_no_model: 0,
        skipped_other_types: 0,
        skipped_effects: 0,
    }
}

/// Load a cell by editor ID (case-insensitive exact match). Returns
/// `None` when no CELL with that EDID exists. `spawn` is where the
/// player arrives (`None` = the default viewpoint); it is set when
/// the load was triggered by walking through a door.
pub fn load_cell(plugin: &Plugin, edid: &str, spawn: Option<SpawnPoint>) -> Option<CellData> {
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    let cell = find_cell(plugin, edid)?;
    let interior = cell_is_interior(cell);
    let index = CellIndex::build(plugin);
    let placed = index.children_of(cell.form_id).to_vec();
    let npc_index = NpcIndex::build(&by_form_id);

    let mut data = empty_cell_data(
        cell.edid().unwrap_or_default(),
        cell.form_id,
        interior,
        spawn,
    );
    append_objects(&placed, &by_form_id, &npc_index, &mut data);
    build_doors(&mut data, &index, &by_form_id);
    Some(data)
}

/// Load a 3x3 block of Tamriel exterior cells centred on `center`
/// (grid coordinates), with their LAND terrain and every placed
/// object — including the persistent refs filed under Tamriel's
/// persistent cell whose positions fall inside the block.
/// Returns `None` when the centre cell does not exist.
pub fn load_exterior(
    plugin: &Plugin,
    center: (i32, i32),
    spawn: Option<SpawnPoint>,
) -> Option<CellData> {
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    let tamriel = TamrielCells::build(plugin)?;
    let center_cell = tamriel.get(center)?;
    let index = CellIndex::build(plugin);
    let npc_index = NpcIndex::build(&by_form_id);

    let mut data = empty_cell_data(
        format!("Riverwood ({}, {})", center.0, center.1),
        center_cell.form_id,
        false,
        spawn,
    );

    // The 3x3 block: LAND + cell-filed refs per cell.
    for gx in center.0 - 1..=center.0 + 1 {
        for gy in center.1 - 1..=center.1 + 1 {
            let Some(cell) = tamriel.get((gx, gy)) else {
                continue;
            };
            let children = index.children_of(cell.form_id);
            if let Some(land) = children.iter().find(|r| r.type_str() == "LAND") {
                if let Some(mut decoded) = decode_land(land, (gx, gy)) {
                    resolve_land_textures(&mut decoded, &by_form_id);
                    data.lands.push(decoded);
                }
            }
            append_objects(children, &by_form_id, &npc_index, &mut data);
        }
    }
    // Water: one surface per cell whose terrain dips below its
    // resolved level (XCLW/XCWT, else the Tamriel DNAM/NAM2
    // defaults). Resolved after the LAND loop so the height
    // check reads the decoded terrain.
    data.waters = resolve_waters(&tamriel, &data.lands, &by_form_id, center);

    // Persistent refs: filed under Tamriel's persistent cell
    // (form 0x00000d74), positioned all over the world. Keep the
    // ones inside this block's rectangle. The cell is found by
    // form ID, never by grid (0,0) — see
    // PERSISTENT_CELL_FORM_ID.
    let min_x = (center.0 - 1) as f32 * CELL_SPAN;
    let max_x = (center.0 + 2) as f32 * CELL_SPAN;
    let min_y = (center.1 - 1) as f32 * CELL_SPAN;
    let max_y = (center.1 + 2) as f32 * CELL_SPAN;
    let persist_children = index.children_of(PERSISTENT_CELL_FORM_ID);
    let in_block: Vec<&Record> = persist_children
        .iter()
        .copied()
        .filter(|r| {
            let kind = r.type_str();
            if kind != "REFR" && kind != "ACHR" {
                return false;
            }
            match r.field(b"DATA").and_then(|f| parse_placement(&f.data)) {
                Some((pos, _)) => {
                    pos[0] >= min_x && pos[0] < max_x && pos[1] >= min_y && pos[1] < max_y
                }
                None => false,
            }
        })
        .collect();
    append_objects(&in_block, &by_form_id, &npc_index, &mut data);
    build_doors(&mut data, &index, &by_form_id);
    Some(data)
}

/// Resolve an exterior block's water surfaces: per cell, the
/// XCLW height and XCWT type with the Tamriel DNAM/NAM2
/// defaults (see `dovah_esm::water`), keeping only cells whose
/// LAND minimum dips below the level — a cell whose terrain
/// stays above the water draws no surface. A cell with no LAND
/// record draws none either (no terrain to dip). Pure over the
/// loaded records; the renderer spawns one flat plane per
/// surface. Riverwood (centre (5,-11)): 6 of the 9 cells get
/// water at -250 units, all RiverWaterFlowNE (see
/// `water_probe.log`).
fn resolve_waters(
    tamriel: &TamrielCells,
    lands: &[LandData],
    by_form_id: &HashMap<u32, &Record>,
    center: (i32, i32),
) -> Vec<WaterSurface> {
    let (default_level, default_type) = by_form_id
        .get(&TAMRIEL_WRLD)
        .map(|w| parse_wrld_water(w))
        .unwrap_or((None, None));
    let mut waters = Vec::new();
    for gx in center.0 - 1..=center.0 + 1 {
        for gy in center.1 - 1..=center.1 + 1 {
            let Some(cell) = tamriel.get((gx, gy)) else {
                continue;
            };
            let (xclw, xcwt) = parse_cell_water(cell);
            let Some(level) = resolve_water_level(xclw, default_level) else {
                continue;
            };
            let Some(watr_id) = xcwt.or(default_type) else {
                continue;
            };
            let Some(watr_rec) = by_form_id.get(&watr_id) else {
                continue;
            };
            let Some(watr) = parse_watr(watr_rec) else {
                continue;
            };
            let dips = lands
                .iter()
                .find(|l| l.grid == (gx, gy))
                .is_some_and(|l| l.height_range().0 < level);
            if !dips {
                continue;
            }
            waters.push(WaterSurface {
                grid: (gx, gy),
                level,
                watr,
            });
        }
    }
    waters
}

/// The spawn point for arriving outside from an interior cell: the
/// first XTEL door in the given cell, resolved to its arrival
/// position and facing. Used by the desktop `--exterior` path so
/// the screenshot starts where the player would step out.
pub fn find_xtel_spawn(plugin: &Plugin, cell_edid: &str) -> Option<SpawnPoint> {
    let cell = load_cell(plugin, cell_edid, None)?;
    cell.doors.first().map(|d| d.spawn)
}

impl CellData {
    /// Median position of the placed objects, in Skyrim units.
    /// Robust against outliers (a basement or an attic full of
    /// objects does not drag the center away from the main room).
    pub fn median_position(&self) -> [f32; 3] {
        fn median(mut values: Vec<f32>) -> f32 {
            if values.is_empty() {
                return 0.0;
            }
            values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mid = values.len() / 2;
            if values.len() % 2 == 1 {
                values[mid]
            } else {
                (values[mid - 1] + values[mid]) / 2.0
            }
        }
        let xs: Vec<f32> = self.objects.iter().map(|o| o.position[0]).collect();
        let ys: Vec<f32> = self.objects.iter().map(|o| o.position[1]).collect();
        let zs: Vec<f32> = self.objects.iter().map(|o| o.position[2]).collect();
        [median(xs), median(ys), median(zs)]
    }

    /// Axis-aligned bounds of all placed positions, Skyrim units.
    pub fn position_bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for o in &self.objects {
            for i in 0..3 {
                min[i] = min[i].min(o.position[i]);
                max[i] = max[i].max(o.position[i]);
            }
        }
        if min[0].is_infinite() {
            min = [0.0; 3];
            max = [0.0; 3];
        }
        (min, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_parses_position_and_rotation() {
        let mut bytes = Vec::new();
        for v in [10.0f32, 20.0, 30.0, 0.1, 0.2, 0.3] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let (pos, rot) = parse_placement(&bytes).unwrap();
        assert_eq!(pos, [10.0, 20.0, 30.0]);
        assert_eq!(rot, [0.1, 0.2, 0.3]);
        assert!(parse_placement(&bytes[..20]).is_none());
        assert!(parse_placement(&[]).is_none());
    }

    #[test]
    fn obnd_parses_six_int16() {
        let mut bytes = Vec::new();
        for v in [-64i16, -32, 0, 64, 32, 96] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let b = parse_obnd(&bytes).unwrap();
        assert_eq!(b.min, [-64, -32, 0]);
        assert_eq!(b.max, [64, 32, 96]);
        assert!(parse_obnd(&bytes[..10]).is_none());
    }

    #[test]
    fn light_parses_radius_and_bbgrr_color() {
        let mut bytes = vec![0u8; 48];
        // time = -1, radius = 512, color = 0x00112233 (BB=0x11, GG=0x22, RR=0x33)
        bytes[0..4].copy_from_slice(&(-1i32).to_le_bytes());
        bytes[4..8].copy_from_slice(&512u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&0x0011_2233u32.to_le_bytes());
        // flags = 0x1009 (shadow-omni + flicker + dynamic),
        // falloff = 1.0 — the inn's FarmInteriorFireLight01
        // values.
        bytes[12..16].copy_from_slice(&0x1009u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&1.0f32.to_le_bytes());
        let light = parse_light(&bytes, 1.5).unwrap();
        assert_eq!(light.radius, 512);
        // 0x00BBGGRR: R is the low byte.
        assert_eq!(light.color, [0x33, 0x22, 0x11]);
        assert_eq!(light.flags, 0x1009);
        assert!(light.flickers());
        // The fireplace casts shadows (M16).
        assert!(light.casts_shadows());
        assert_eq!(light.falloff, 1.0);
        // Fade is the caller's FNAM value, passed through.
        assert_eq!(light.fade, 1.5);
        assert!(parse_light(&bytes[..8], 1.0).is_none());
        assert!(parse_light(&bytes[..16], 1.0).is_none());
    }

    #[test]
    fn shadow_rule_is_the_omni_type_flag_only() {
        // A sconce (0x9: flicker + dynamic, no type flag) does
        // not cast shadows; a shadow SPOTLIGHT (0x401) and a
        // shadow HEMISPHERE (0x801) are not point lights, so
        // they do not cast point shadows here either; a plain
        // shadow-omni light (0x1000, no effect bits) does.
        let sconce = LightData {
            radius: 512,
            color: [0; 3],
            flags: 0x9,
            falloff: 1.0,
            fade: 1.0,
        };
        assert!(!sconce.casts_shadows());
        let spotlight = LightData {
            flags: 0x401,
            ..sconce
        };
        assert!(!spotlight.casts_shadows());
        let hemisphere = LightData {
            flags: 0x801,
            ..sconce
        };
        assert!(!hemisphere.casts_shadows());
        let omni = LightData {
            flags: 0x1000,
            ..sconce
        };
        assert!(omni.casts_shadows());
    }

    #[test]
    fn interior_ligh_refs_survive_without_a_model() {
        use dovah_esm::Field;

        // A model-less LIGH base (no MODL — pure light, like all
        // 8 of the inn's lights) and a REFR pointing at it.
        let mut light_data = vec![0u8; 48];
        light_data[0..4].copy_from_slice(&(-1i32).to_le_bytes());
        light_data[4..8].copy_from_slice(&512u32.to_le_bytes());
        light_data[8..12].copy_from_slice(&0x0064_a2c6u32.to_le_bytes());
        light_data[12..16].copy_from_slice(&0x0009u32.to_le_bytes());
        light_data[16..20].copy_from_slice(&1.0f32.to_le_bytes());
        let base = Record {
            record_type: *b"LIGH",
            flags: 0,
            form_id: 0x00059a93,
            version: 44,
            fields: vec![Field {
                field_type: *b"DATA",
                data: light_data,
            }],
        };
        let mut placement = Vec::new();
        for v in [-771.5f32, -767.6, 180.2, 0.0, 0.0, 0.0] {
            placement.extend_from_slice(&v.to_le_bytes());
        }
        let refr = Record {
            record_type: *b"REFR",
            flags: 0,
            form_id: 0x0005ed8f,
            version: 44,
            fields: vec![
                Field {
                    field_type: *b"DATA",
                    data: placement,
                },
                Field {
                    field_type: *b"NAME",
                    data: 0x00059a93u32.to_le_bytes().to_vec(),
                },
            ],
        };
        let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
        by_form_id.insert(base.form_id, &base);
        let npc_index = NpcIndex::build(&by_form_id);

        // Interior: the ref survives with its light attached.
        let mut interior = empty_cell_data("Inn".to_string(), 0x133c6, true, None);
        append_objects(&[&refr], &by_form_id, &npc_index, &mut interior);
        assert_eq!(interior.objects.len(), 1);
        assert_eq!(interior.skipped_no_model, 0);
        let light = interior.objects[0].light.expect("light parsed");
        assert_eq!(light.radius, 512);
        assert_eq!(light.color, [0xc6, 0xa2, 0x64]);
        assert!(light.flickers());

        // Exterior: the same ref is still dropped — M14 is
        // interior-only, and the exterior must stay untouched.
        let mut exterior = empty_cell_data("Tamriel".to_string(), 0x3c, false, None);
        append_objects(&[&refr], &by_form_id, &npc_index, &mut exterior);
        assert_eq!(exterior.objects.len(), 0);
        assert_eq!(exterior.skipped_no_model, 1);
    }

    #[test]
    fn xesp_enable_parent_chain() {
        use dovah_esm::Field;
        // The naked-guard rule: a ref with a disabled enable
        // parent does not spawn; an enabled parent lets it
        // spawn; the "opposite of parent" flag flips it; a
        // missing parent keeps the current behavior (enabled).
        fn xesp_record(form_id: u32, flags: u32, parent: Option<(u32, u8)>) -> Record {
            let mut fields = Vec::new();
            if let Some((parent_id, opposite)) = parent {
                let mut data = parent_id.to_le_bytes().to_vec();
                data.push(opposite);
                fields.push(Field {
                    field_type: *b"XESP",
                    data,
                });
            }
            Record {
                record_type: *b"REFR",
                flags,
                form_id,
                version: 44,
                fields,
            }
        }
        let disabled_parent = xesp_record(0x1b753, FLAG_INITIALLY_DISABLED, None);
        let enabled_parent = xesp_record(0x1b754, 0, None);
        let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
        by_form_id.insert(disabled_parent.form_id, &disabled_parent);
        by_form_id.insert(enabled_parent.form_id, &enabled_parent);

        // Stormcloak guard: disabled parent, no opposite flag.
        let stormcloak = xesp_record(0x1b74f, 0, Some((0x1b753, 0)));
        assert!(!ref_enabled(&stormcloak, &by_form_id, 0));
        // Imperial guard: enabled parent.
        let imperial = xesp_record(0x1b743, 0, Some((0x1b754, 0)));
        assert!(ref_enabled(&imperial, &by_form_id, 0));
        // Opposite flag: disabled parent + opposite = enabled.
        let flipped = xesp_record(0x1b750, 0, Some((0x1b753, 1)));
        assert!(ref_enabled(&flipped, &by_form_id, 0));
        // Opposite flag: enabled parent + opposite = disabled.
        let flipped2 = xesp_record(0x1b751, 0, Some((0x1b754, 1)));
        assert!(!ref_enabled(&flipped2, &by_form_id, 0));
        // Missing parent (DLC record we do not load): enabled.
        let orphan = xesp_record(0x1b752, 0, Some((0xdeadbeef, 0)));
        assert!(ref_enabled(&orphan, &by_form_id, 0));
        // No XESP at all: the flag alone decides.
        let plain = xesp_record(0x1b755, 0, None);
        assert!(ref_enabled(&plain, &by_form_id, 0));
        let plain_disabled = xesp_record(0x1b756, FLAG_INITIALLY_DISABLED, None);
        assert!(!ref_enabled(&plain_disabled, &by_form_id, 0));
        // Chain: a child of the Stormcloak guard inherits the
        // disabled state through two levels.
        let grandchild = xesp_record(0x1b757, 0, Some((0x1b74f, 0)));
        by_form_id.insert(stormcloak.form_id, &stormcloak);
        assert!(!ref_enabled(&grandchild, &by_form_id, 0));
    }

    #[test]
    fn xtel_parses_32_bytes() {
        // The inn door's real XTEL (door_pair example): dest
        // 0x00013424, pos (23259.8, -44476.2, -8.3), rot
        // (0, 0, -0.842), tail 0.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x00013424u32.to_le_bytes());
        for v in [23259.8f32, -44476.2, -8.3, 0.0, 0.0, -0.842] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(bytes.len(), 32);
        let xtel = parse_xtel(&bytes).unwrap();
        assert_eq!(xtel.dest_ref, 0x00013424);
        assert_eq!(xtel.pos, [23259.8, -44476.2, -8.3]);
        assert_eq!(xtel.rot[2], -0.842);
        // The old 28-byte reading is rejected, not misread.
        assert!(parse_xtel(&bytes[..28]).is_none());
        assert!(parse_xtel(&[]).is_none());
    }

    #[test]
    fn xclc_parses_12_bytes() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&5i32.to_le_bytes());
        bytes.extend_from_slice(&(-11i32).to_le_bytes());
        bytes.extend_from_slice(&0x0053fd00u32.to_le_bytes());
        assert_eq!(parse_xclc(&bytes), Some((5, -11)));
        // The old 8-byte reading is rejected.
        assert!(parse_xclc(&bytes[..8]).is_none());
    }

    #[test]
    fn grid_of_position_floors_to_the_cell() {
        // The inn door's XTEL arrival point is in grid (5,-11).
        assert_eq!(grid_of_position([23259.8, -44476.2, -8.3]), (5, -11));
        // Exact cell corners belong to the cell they start.
        assert_eq!(grid_of_position([20480.0, -45056.0, 0.0]), (5, -11));
        // Just below a boundary belongs to the cell below it.
        assert_eq!(grid_of_position([20479.9, -45056.0, 0.0]), (4, -11));
    }

    #[test]
    fn spawn_point_comes_from_xtel_arrival() {
        let xtel = XtelTarget {
            dest_ref: 0x1234,
            pos: [100.0, 200.0, 300.0],
            rot: [0.0, 0.0, 1.5],
        };
        let spawn = SpawnPoint::from(xtel);
        assert_eq!(spawn.position, [100.0, 200.0, 300.0]);
        assert_eq!(spawn.yaw, 1.5);
    }

    #[test]
    fn grid_zero_zero_is_the_geographic_cell_not_the_persistent_cell() {
        use dovah_esm::{Field, GroupChild};

        fn cell(form_id: u32, gx: i32, gy: i32) -> Record {
            let mut data = Vec::new();
            data.extend_from_slice(&gx.to_le_bytes());
            data.extend_from_slice(&gy.to_le_bytes());
            data.extend_from_slice(&0x0053fd00u32.to_le_bytes());
            Record {
                record_type: *b"CELL",
                flags: 0,
                form_id,
                version: 44,
                fields: vec![Field {
                    field_type: *b"XCLC",
                    data,
                }],
            }
        }

        // Real Skyrim.esm has TWO Tamriel CELLs at grid (0,0): the
        // persistent cell (0x00000d74, 15,196 children) and the
        // geographic cell BleakwindBasinExterior01 (0x000099a2).
        // The persistent cell goes LAST here — the visit order
        // that used to win the grid map and hide every persistent
        // ref, so the M9 exterior loaded with 0 doors.
        let tg = Group {
            label: TAMRIEL_WRLD.to_le_bytes(),
            group_type: GroupType::WorldChildren,
            children: vec![
                GroupChild::Record(cell(0x000099a2, 0, 0)),
                GroupChild::Record(cell(PERSISTENT_CELL_FORM_ID, 0, 0)),
            ],
        };
        let plugin = Plugin {
            header: Record {
                record_type: *b"TES4",
                flags: 0,
                form_id: 0,
                version: 44,
                fields: Vec::new(),
            },
            groups: vec![tg],
        };
        let tamriel = TamrielCells::build(&plugin).expect("Tamriel group");
        assert_eq!(tamriel.get((0, 0)).unwrap().form_id, 0x000099a2);
    }

    #[test]
    fn xtel_targets_resolve_through_the_inn_door_pair() {
        use dovah_esm::{Field, GroupChild};

        fn record(form_id: u32, kind: &[u8; 4], fields: Vec<Field>) -> Record {
            Record {
                record_type: *kind,
                flags: 0,
                form_id,
                version: 44,
                fields,
            }
        }

        fn edid_field(name: &str) -> Field {
            let mut data = name.as_bytes().to_vec();
            data.push(0);
            Field {
                field_type: *b"EDID",
                data,
            }
        }

        // The real door pair (door_pair.rs): interior door ref
        // 0x00013419, child of the inn CELL 0x000133c6 (interior,
        // EDID RiverwoodSleepingGiantInn); exterior door ref
        // 0x00013424, filed under the persistent cell.
        let inn_cell = record(
            0x000133c6,
            b"CELL",
            vec![
                edid_field("RiverwoodSleepingGiantInn"),
                Field {
                    field_type: *b"DATA",
                    data: vec![0x01],
                },
            ],
        );
        let interior_door = record(0x00013419, b"REFR", vec![]);
        let exterior_door = record(0x00013424, b"REFR", vec![]);
        let plugin = Plugin {
            header: record(0, b"TES4", vec![]),
            groups: vec![
                Group {
                    label: *b"CELL",
                    group_type: GroupType::Top,
                    children: vec![GroupChild::Record(inn_cell)],
                },
                Group {
                    label: 0x000133c6u32.to_le_bytes(),
                    group_type: GroupType::CellChildren,
                    children: vec![GroupChild::Record(interior_door)],
                },
                Group {
                    label: PERSISTENT_CELL_FORM_ID.to_le_bytes(),
                    group_type: GroupType::CellPersistentChildren,
                    children: vec![GroupChild::Record(exterior_door)],
                },
            ],
        };
        let index = CellIndex::build(&plugin);
        let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
        for r in plugin.all_records() {
            by_form_id.insert(r.form_id, r);
        }

        // The exterior door's XTEL points at the interior door
        // ref, whose parent cell is the inn → Interior target.
        let out_xtel = XtelTarget {
            dest_ref: 0x00013419,
            pos: [-250.3, -360.4, 0.0],
            rot: [0.0, 0.0, -0.008],
        };
        assert_eq!(
            resolve_xtel_target(&index, &by_form_id, &out_xtel),
            SceneTarget::Interior {
                form_id: 0x000133c6,
                edid: "RiverwoodSleepingGiantInn".to_string()
            }
        );
        // The interior door's XTEL points at the exterior door
        // ref, filed under the persistent cell (not interior) →
        // Exterior target centred on the XTEL position's grid.
        let in_xtel = XtelTarget {
            dest_ref: 0x00013424,
            pos: [23259.8, -44476.2, -8.3],
            rot: [0.0, 0.0, -0.842],
        };
        assert_eq!(
            resolve_xtel_target(&index, &by_form_id, &in_xtel),
            SceneTarget::Exterior { grid: (5, -11) }
        );
    }
}
