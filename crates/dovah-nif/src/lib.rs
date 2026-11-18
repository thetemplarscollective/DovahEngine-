//! NIF mesh loader for Dovahengine.
//!
//! Reads NetImmerse File Format (NIF) meshes from Skyrim Special
//! Edition (NIF 20.2.0.7, BS stream version 100) and Skyrim LE (BS
//! stream version 83), following the public NifTools format
//! specification (`docs/formats/nif.md`). This crate contains no
//! Bethesda code and ships no game data: it parses mesh files the user
//! already owns.
//!
//! The parser walks the file's block table (using the header block
//! sizes, so unknown block types are skipped safely) and assembles
//! plain mesh data — vertices, normals, UVs, triangle indices,
//! material names, and texture paths — via [`NifFile::meshes`].
//! Skinning blocks (`NiSkinInstance`, `NiSkinData`,
//! `NiSkinPartition`, `BSDismemberSkinInstance`) are parsed in
//! their Skyrim SE layouts (M12) and skinned shapes are
//! reconstructed in bind pose — see [`NifFile::meshes`].
//! Animation blocks (`NiControllerManager`,
//! `NiTransformController`, .hkx) are still skipped: bind pose
//! only, no animation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;

/// NIF version used by Skyrim LE and SE: 20.2.0.7.
pub const NIF_VERSION_SKYRIM: u32 = 0x1402_0007;
/// BS stream version for Skyrim LE.
pub const BS_VERSION_SKYRIM_LE: u32 = 83;
/// BS stream version for Skyrim SE.
pub const BS_VERSION_SKYRIM_SE: u32 = 100;
/// User version for Skyrim (LE and SE).
pub const USER_VERSION_SKYRIM: u32 = 12;

/// A 2D vector (UV coordinate).
pub type Vec2 = [f32; 2];
/// A 3D vector (position, normal, tangent).
pub type Vec3 = [f32; 3];
/// A 4D vector (sparkle parameters).
pub type Vec4 = [f32; 4];
/// An RGB color (3 floats).
pub type Color3 = [f32; 3];
/// An RGBA color (4 floats).
pub type Color4 = [f32; 4];
/// A 3x3 rotation matrix, row-major as stored in the file.
pub type Mat3 = [[f32; 3]; 3];

#[derive(Debug)]
pub enum NifError {
    Io(io::Error),
    /// The file does not start with a Gamebryo header string.
    NotANif,
    /// A NIF version or BS stream version this reader does not
    /// support (only 20.2.0.7 with BS version 83 or 100).
    UnsupportedVersion {
        version: u32,
        user_version: u32,
        bs_version: u32,
    },
    /// The file is big-endian; Skyrim NIFs are little-endian.
    UnsupportedEndian,
    Truncated(&'static str),
    InvalidData(&'static str),
}

impl std::fmt::Display for NifError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NifError::Io(e) => write!(f, "I/O error: {e}"),
            NifError::NotANif => {
                write!(f, "not a NIF file (missing Gamebryo header string)")
            }
            NifError::UnsupportedVersion {
                version,
                user_version,
                bs_version,
            } => write!(
                f,
                "unsupported NIF version 0x{version:08x} (user {user_version}, BS stream {bs_version}); only 20.2.0.7 with BS stream 83 or 100 is supported"
            ),
            NifError::UnsupportedEndian => {
                write!(f, "big-endian NIF files are not supported")
            }
            NifError::Truncated(what) => write!(f, "truncated NIF in {what}"),
            NifError::InvalidData(what) => write!(f, "invalid NIF data: {what}"),
        }
    }
}

impl std::error::Error for NifError {}

impl From<io::Error> for NifError {
    fn from(e: io::Error) -> Self {
        NifError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, NifError>;

/// The `NiObjectNET` base: name, extra data, and controller refs.
/// The name is resolved from the header string table at parse time.
#[derive(Debug, Clone, Default)]
pub struct ObjectNet {
    pub name: String,
    pub extra_data: Vec<i32>,
    pub controller: i32,
}

/// The `NiAVObject` base: an object with a local transform in the
/// scene graph.
#[derive(Debug, Clone)]
pub struct AvObject {
    pub net: ObjectNet,
    pub flags: u32,
    pub translation: Vec3,
    pub rotation: Mat3,
    pub scale: f32,
    pub collision_object: i32,
}

/// The `MaterialData` struct on `NiGeometry`: the material names a
/// shape can use, and which one is active.
#[derive(Debug, Clone, Default)]
pub struct MaterialData {
    pub names: Vec<String>,
    pub extra_data: Vec<i32>,
    pub active_material: i32,
    pub needs_update: bool,
}

/// The `NiGeometry` base shared by `NiTriShape`, `NiTriStrips`, and
/// `BSLODTriShape`.
#[derive(Debug, Clone)]
pub struct Geometry {
    pub av: AvObject,
    /// Index of the `NiTriShapeData` / `NiTriStripsData` block, or -1.
    pub data: i32,
    /// Index of the `NiSkinInstance` block, or -1 (skinning is future
    /// work; the mesh assembler ignores this).
    pub skin_instance: i32,
    pub material_data: MaterialData,
    /// Index of the `BSLightingShaderProperty` block, or -1.
    pub shader_property: i32,
    /// Index of the `NiAlphaProperty` block, or -1.
    pub alpha_property: i32,
}

/// A `NiNode`: a scene-graph node grouping children.
#[derive(Debug, Clone)]
pub struct NodeBlock {
    pub av: AvObject,
    pub children: Vec<i32>,
    pub effects: Vec<i32>,
}

/// A `NiSwitchNode` (M13): a NiNode whose children are
/// alternatives — only the ACTIVE child renders. The real engine
/// switches the active child at runtime (near/far LOD); our
/// static renderer renders the stored active child (child 0 for
/// every pine NIF: the full-detail tree, with the LOD tree as
/// the inactive child). Layout is a NiNode prefix plus a u16
/// flags field and an i32 active-child index (6 trailing bytes,
/// verified on treePineForest01.nif: flags 3, index 0).
#[derive(Debug, Clone)]
pub struct SwitchNodeBlock {
    pub node: NodeBlock,
    /// Stored active-child index (into `node.children`).
    pub active_child: i32,
}

/// A `NiTriShape`: a shape whose data lives in a `NiTriShapeData`
/// block.
#[derive(Debug, Clone)]
pub struct TriShapeBlock {
    pub geometry: Geometry,
}

/// A `NiTriStrips`: a shape whose data lives in a `NiTriStripsData`
/// block (triangle strips, triangulated by the mesh assembler).
#[derive(Debug, Clone)]
pub struct TriStripsBlock {
    pub geometry: Geometry,
}

/// A `BSLODTriShape`: a `NiTriShape` variant with three LOD sizes.
#[derive(Debug, Clone)]
pub struct BsLodTriShapeBlock {
    pub geometry: Geometry,
    pub lod_sizes: [u32; 3],
}

/// The `NiGeometryData` base: vertex attributes shared by
/// `NiTriShapeData` and `NiTriStripsData`. Arrays are empty when the
/// corresponding data is absent in the file.
#[derive(Debug, Clone, Default)]
pub struct GeometryData {
    pub group_id: i32,
    pub keep_flags: u8,
    pub compress_flags: u8,
    pub vertices: Vec<Vec3>,
    /// `BSGeometryDataFlags`: bit 0 = Has UV, bit 12 = Has Tangents.
    pub bs_data_flags: u16,
    pub material_crc: u32,
    pub normals: Vec<Vec3>,
    pub tangents: Vec<Vec3>,
    pub bitangents: Vec<Vec3>,
    pub bounding_sphere: (Vec3, f32),
    pub vertex_colors: Vec<Color4>,
    /// The first (and for Skyrim, only) UV set.
    pub uvs: Vec<Vec2>,
    pub consistency_flags: u16,
    pub additional_data: i32,
}

/// A `NiTriShapeData` block: mesh data as a triangle list.
#[derive(Debug, Clone)]
pub struct TriShapeDataBlock {
    pub data: GeometryData,
    pub num_triangles: u16,
    pub triangles: Vec<[u16; 3]>,
    /// Shared-normal vertex groups; parsed for completeness, not used
    /// by the mesh assembler.
    pub match_groups: Vec<Vec<u16>>,
}

/// A `NiTriStripsData` block: mesh data as triangle strips.
#[derive(Debug, Clone)]
pub struct TriStripsDataBlock {
    pub data: GeometryData,
    pub num_triangles: u16,
    pub strips: Vec<Vec<u16>>,
}

/// One vertex of a `BSTriShape`, as described by its `BSVertexDesc`.
/// Each attribute is `Some` when the corresponding attribute bit is
/// set in the vertex description, `None` otherwise. Attributes are
/// uniform across a block, so every vertex in one shape has the same
/// set of `Some` fields.
#[derive(Debug, Clone, Default)]
pub struct BsVertex {
    pub position: Option<Vec3>,
    /// First bitangent component; present with tangents.
    pub bitangent_x: Option<f32>,
    pub uv: Option<Vec2>,
    /// Byte normal decoded to [-1, 1].
    pub normal: Option<Vec3>,
    /// Second bitangent component; present with normals.
    pub bitangent_y: Option<f32>,
    /// Byte tangent decoded to [-1, 1].
    pub tangent: Option<Vec3>,
    /// Third bitangent component; present with tangents.
    pub bitangent_z: Option<f32>,
    /// Vertex color bytes (0-255), present with the color bit.
    pub color: Option<[u8; 4]>,
    pub eye_data: Option<f32>,
}

/// A `BSDynamicTriShape` block: the SE-native shape for meshes that
/// morph at runtime (character heads and CharGen parts). It is a
/// `BSTriShape` plus a dynamic section: the standard vertex data is
/// empty (`data_size` 0 — the triangles and attributes live in the
/// shape's `NiSkinPartition`, which the loader does not parse yet),
/// and the vertex *positions* live in a separate dynamic array after
/// the particle data: a u32 size, then one record per vertex, each
/// `stride` bytes (vertex desc bits 4-7, × 4; 16 in every real file
/// swept). The first 12 bytes of each record are the position; the
/// remaining bytes (4 per vertex, one float in [-1, 1] on beards and
/// 0 on heads) are uninterpreted and skipped.
/// `trailing_bytes` records any bytes left after the dynamic
/// section: 0 in every real file swept in M6.1 (21,132 blocks).
#[derive(Debug, Clone)]
pub struct BsDynamicTriShapeBlock {
    pub shape: BsTriShapeBlock,
    /// Vertex positions from the dynamic section, in vertex order.
    /// Empty when the shape carries no dynamic section.
    pub dynamic_positions: Vec<Vec3>,
    /// The dynamic section's declared size in bytes (0 when absent).
    pub dynamic_data_size: u32,
    pub trailing_bytes: usize,
}

/// A `BSTriShape` block: the SE-native shape, with vertex data inline.
/// Most Skyrim SE meshes use this.
#[derive(Debug, Clone)]
pub struct BsTriShapeBlock {
    pub av: AvObject,
    pub bounding_sphere: (Vec3, f32),
    /// Index of the skin instance block, or -1 (skinning is future
    /// work).
    pub skin: i32,
    /// Index of the `BSLightingShaderProperty` block, or -1.
    pub shader_property: i32,
    /// Index of the `NiAlphaProperty` block, or -1.
    pub alpha_property: i32,
    /// Raw `BSVertexDesc` (u64): vertex size, attribute offsets, and
    /// the attribute flags in bits 44-55.
    pub vertex_desc: u64,
    /// Declared triangle / vertex counts from the header. For skinned
    /// SSE meshes these can be 0 even though the mesh has geometry:
    /// the data lives in the shape's `NiSkinPartition` instead.
    pub num_triangles: u16,
    pub num_vertices: u16,
    pub vertices: Vec<BsVertex>,
    pub triangles: Vec<[u16; 3]>,
    /// Half-precision copy of the mesh the game uses for its particle
    /// system. Parsed, but the mesh assembler ignores it.
    pub particle_vertices: Vec<Vec3>,
    pub particle_normals: Vec<Vec3>,
    pub particle_triangles: Vec<[u16; 3]>,
}

/// A UV transform on a texture slot (`NiTextureTransform`).
#[derive(Debug, Clone, Copy)]
pub struct TexTransform {
    pub translation: Vec2,
    pub scale: Vec2,
    pub rotation: f32,
    pub method: u32,
    pub center: Vec2,
}

/// A `TexDesc`: one texture slot of a `NiTexturingProperty`.
#[derive(Debug, Clone)]
pub struct TexDesc {
    /// Index of the `NiSourceTexture` block.
    pub source: i32,
    pub flags: u16,
    pub transform: Option<TexTransform>,
}

/// The bump-map slot: a texture plus its luma scale/offset/matrix.
#[derive(Debug, Clone)]
pub struct BumpMap {
    pub map: TexDesc,
    pub luma_scale: f32,
    pub luma_offset: f32,
    pub matrix: [f32; 4],
}

/// The parallax slot: a texture plus its offset.
#[derive(Debug, Clone)]
pub struct ParallaxMap {
    pub map: TexDesc,
    pub offset: f32,
}

/// A `ShaderTexDesc`: one shader texture entry.
#[derive(Debug, Clone)]
pub struct ShaderTexDesc {
    pub map: Option<TexDesc>,
    pub map_id: Option<u32>,
}

/// A `NiTexturingProperty` block: the legacy Gamebryo texture slots.
/// Skyrim meshes normally use `BSLightingShaderProperty` instead;
/// this is parsed when present but the mesh assembler does not use
/// it for texture paths.
#[derive(Debug, Clone)]
pub struct TexturingPropertyBlock {
    pub net: ObjectNet,
    pub flags: u16,
    pub texture_count: u32,
    pub base: Option<TexDesc>,
    pub dark: Option<TexDesc>,
    pub detail: Option<TexDesc>,
    pub gloss: Option<TexDesc>,
    pub glow: Option<TexDesc>,
    pub bump_map: Option<BumpMap>,
    pub normal: Option<TexDesc>,
    pub parallax: Option<ParallaxMap>,
    pub decals: [Option<TexDesc>; 4],
    pub shader_textures: Vec<ShaderTexDesc>,
}

/// `NiTexture::FormatPrefs`: renderer hints, not requirements.
#[derive(Debug, Clone, Copy, Default)]
pub struct FormatPrefs {
    pub pixel_layout: u32,
    pub use_mipmaps: u32,
    pub alpha_format: u32,
}

/// A `NiSourceTexture` block: a reference to a texture file.
#[derive(Debug, Clone)]
pub struct SourceTextureBlock {
    pub net: ObjectNet,
    /// 1 = external file, 0 = embedded pixel data.
    pub use_external: u8,
    /// The texture path (external) or the original source filename
    /// (embedded). Empty when the file stores no name.
    pub file_name: String,
    pub pixel_data: i32,
    pub format_prefs: FormatPrefs,
    pub is_static: u8,
    pub direct_render: bool,
    pub persist_render_data: bool,
}

/// The conditional trailing fields of a `BSLightingShaderProperty`,
/// by shader type. Types with no extra fields use `None`.
#[derive(Debug, Clone)]
pub enum ShaderTypeExtra {
    None,
    /// Shader type 1 (Environment Map).
    EnvironmentMap {
        env_map_scale: f32,
    },
    /// Shader type 5 (Skin Tint).
    SkinTint {
        color: Color3,
    },
    /// Shader type 6 (Hair Tint).
    HairTint {
        color: Color3,
    },
    /// Shader type 7 (Parallax Occ).
    ParallaxOcc {
        max_passes: f32,
        scale: f32,
    },
    /// Shader type 11 (MultiLayer Parallax).
    MultiLayerParallax {
        inner_layer_thickness: f32,
        refraction_scale: f32,
        inner_layer_texture_scale: Vec2,
        envmap_strength: f32,
    },
    /// Shader type 14 (Sparkle Snow).
    SparkleSnow {
        sparkle_parameters: Vec4,
    },
    /// Shader type 16 (Eye Envmap).
    EyeEnvmap {
        cubemap_scale: f32,
        left_eye_reflection_center: Vec3,
        right_eye_reflection_center: Vec3,
    },
}

/// A `BSLightingShaderProperty` block: the Skyrim shader.
#[derive(Debug, Clone)]
pub struct LightingShaderPropertyBlock {
    pub net: ObjectNet,
    /// `BSLightingShaderType` (0 Default, 1 Environment Map, ...).
    /// Stored first in the block, before the name.
    pub shader_type: u32,
    pub shader_flags_1: u32,
    pub shader_flags_2: u32,
    pub uv_offset: Vec2,
    pub uv_scale: Vec2,
    /// Index of the `BSShaderTextureSet` block, or -1.
    pub texture_set: i32,
    pub emissive_color: Color3,
    pub emissive_multiple: f32,
    pub texture_clamp_mode: u32,
    pub alpha: f32,
    pub refraction_strength: f32,
    pub glossiness: f32,
    pub specular_color: Color3,
    pub specular_strength: f32,
    pub lighting_effect_1: f32,
    pub lighting_effect_2: f32,
    pub extra: ShaderTypeExtra,
}

/// A `BSShaderTextureSet` block: the texture paths for a shader.
/// Slots: 0 diffuse, 1 normal/gloss, 2 glow/skin/hair/rim, 3
/// height/parallax, 4 environment, 5 environment mask, 6 subsurface,
/// 7 backlighting.
#[derive(Debug, Clone, Default)]
pub struct ShaderTextureSetBlock {
    pub textures: Vec<String>,
}

/// A `NiAlphaProperty` block: transparency flags + threshold.
#[derive(Debug, Clone)]
pub struct AlphaPropertyBlock {
    pub net: ObjectNet,
    pub flags: u16,
    pub threshold: u8,
}

/// A `BSEffectShaderProperty` block: the shader for effects —
/// fog, water sheets, glows, magic. Decoded from the real bytes
/// of `meshes/effects/fxwaterfallskirttallfront.nif` (blocks [4]
/// and [22], 160 B and 125 B — the layout below accounts for
/// every byte of both). Unlike the lighting shader it has NO
/// shader-type prefix: the `NiObjectNET` comes first, then the
/// two flag words, UV offset/scale, the source texture, 44 bytes
/// of numerics, and CONDITIONALLY a second (gradient) texture
/// gated by a shader-flags bit (flags1 0x78 has it, 0x48 does
/// not — the gating bit is unidentified, so the parser reads the
/// second string only when its length exactly fills the
/// remaining block bytes and treats a trailing float as absent).
/// The four floats before the emissive color and the two after
/// it are stored as unknowns — their meanings are unverified.
/// The renderer treats effect shapes as UNLIT: their color comes
/// from the emissive color and the vertex colors, not the scene
/// lights.
#[derive(Debug, Clone)]
pub struct EffectShaderPropertyBlock {
    pub net: ObjectNet,
    pub shader_flags_1: u32,
    pub shader_flags_2: u32,
    pub uv_offset: Vec2,
    pub uv_scale: Vec2,
    /// The effect texture (e.g. `textures\effects\FXFogHeavy.dds`).
    pub source_texture: String,
    /// Texture clamp mode; low byte 0x03 = WRAP_S_WRAP_T. The
    /// 0xa5 byte seen in both fog blocks is unexplained.
    pub texture_clamp_mode: u32,
    /// Four floats between the clamp mode and the emissive color
    /// (values 0.0/0.0/1.0/0.0 in the fog blocks). Unverified.
    pub unknown_pre_emissive: [f32; 4],
    /// Emissive color + alpha (Color4 at +20 from the end of the
    /// source-texture string).
    pub emissive: Color4,
    /// Two floats after the emissive color (0.75/50.0 and
    /// 0.85/100.0 in the fog blocks). Unverified.
    pub unknown_post_emissive: [f32; 2],
    /// The optional gradient texture (e.g.
    /// `textures\effects\gradients\GradCobWeb.dds`). Stored, not
    /// used — Bevy's `StandardMaterial` has one base-color slot.
    pub gradient_texture: Option<String>,
}

/// A `NiMaterialProperty` block: legacy material colors.
#[derive(Debug, Clone)]
pub struct MaterialPropertyBlock {
    pub net: ObjectNet,
    pub specular_color: Color3,
    pub emissive_color: Color3,
    pub glossiness: f32,
    pub alpha: f32,
    pub emissive_mult: f32,
}

/// A `NiSkinInstance` / `BSDismemberSkinInstance` block: the link
/// from a skinned shape to its skin data, skin partition, and bones.
/// M12: the Skyrim SE layout is
/// `Data, SkinPartition, Root, NumBones, Bones` — the partition ref
/// between Data and Root is the SE addition; old docs showing
/// Data/Root/Bones are wrong for SE. All three block types inherit
/// `NiObject`, NOT `NiObjectNET`: no name / extra-data / controller
/// header. Proven byte-exact on `MaleBody_1.nif` (both instances
/// end with remaining = 0).
#[derive(Debug, Clone)]
pub struct SkinInstanceBlock {
    /// Index of the `NiSkinData` block.
    pub data: i32,
    /// Index of the `NiSkinPartition` block.
    pub partition: i32,
    /// Index of the skeleton root node.
    pub root: i32,
    /// Bone node indices, in `NiSkinData` bone-list order.
    pub bones: Vec<i32>,
    /// True when the block is a `BSDismemberSkinInstance`.
    pub dismember: bool,
    /// Dismember partitions as (flags, body-part id) pairs; empty
    /// for a plain `NiSkinInstance`. Parsed and stored, not used for
    /// bind pose.
    pub dismember_partitions: Vec<(u16, u16)>,
}

/// One bone of a `NiSkinData` block: its bind-pose transform in
/// skin space, its bounding sphere, and its (LE-style) vertex
/// weights. The SE bind-pose reconstruction uses only `transform`;
/// the weights live in the skin partition for SE files.
#[derive(Debug, Clone)]
pub struct SkinBone {
    pub transform: Transform,
    pub bounding_sphere: (Vec3, f32),
    pub weights: Vec<(u16, f32)>,
}

/// A `NiSkinData` block (Skyrim SE layout, proven byte-exact):
/// skin transform, bone count, ONE byte (value 1 in every file
/// checked — purpose unknown, parsed and stored), then one
/// [`SkinBone`] per bone. There is NO trailing skin-partition ref
/// in SE — the block ends after the last bone's weights (the LE
/// layout's trailing ref is absent; do not read one).
#[derive(Debug, Clone)]
pub struct SkinDataBlock {
    /// Geometry-space -> skin-space transform.
    pub skin_transform: Transform,
    /// The byte after the bone count; 0x01 in every file checked.
    pub unknown_byte: u8,
    pub bones: Vec<SkinBone>,
    /// Bytes left after parsing (0 in every file checked; non-zero
    /// means a layout variant — the sweep reports it).
    pub trailing_bytes: usize,
}

/// One partition of a `NiSkinPartition` block (Skyrim SE layout).
/// Triangles index GLOBAL vertex indices (proven: part 0 of the
/// body partition has 1,275 local verts but triangles reach index
/// 1,384 = global count - 1); the vertex map maps partition-local
/// weight entries to global vertices. Partitions overlap at
/// dismember seams (1,429 local verts vs 1,385 global on the body),
/// so the assembler takes the first partition's weights per vertex.
#[derive(Debug, Clone)]
pub struct SkinPartitionData {
    /// Bone slots used by this partition (indices into the skin
    /// instance's bone list).
    pub bones: Vec<u16>,
    /// Partition-local vertex -> global vertex index.
    pub vertex_map: Vec<u16>,
    /// Triangles in GLOBAL vertex indices.
    pub triangles: Vec<[u16; 3]>,
    /// Per-local-vertex bone weights as (instance bone index,
    /// weight) pairs, resolved through `bones` at parse time.
    /// Zero-weight entries are dropped.
    pub weights: Vec<Vec<(u16, f32)>>,
}

/// A `NiSkinPartition` block (Skyrim SE layout, proven byte-exact
/// on `MaleBody_1.nif` and all six partitions of Delphine's
/// FaceGen head, every parse ending with remaining = 0):
///
/// ```text
/// NumPartitions (u32)
/// VertexDataSize (u32) — total bytes of the shape's FULL vertex array
/// VertexSize (u32) — stride
/// VertexDesc (u64) — BSVertexDesc, same value as the shape's
/// VertexData (VertexDataSize bytes of BSVertexDataSSE records)
/// per partition:
///   NumVertices/NumTriangles/NumBones/NumStrips/NumWeightsPerVertex (u16 each)
///   Bones (u16[NumBones])
///   HasVertexMap (u8), VertexMap (u16[NumVertices])
///   HasVertexWeights (u8), VertexWeights (f32[NumVertices][NumWeightsPerVertex])
///   StripLengths (u16[NumStrips])
///   HasFaces (u8), Triangles (u16[NumTriangles][3]) or strips
///   HasBoneIndices (u8), BoneIndices (u8[NumVertices][NumWeightsPerVertex])
///   UnknownShort (u16) — 0 in all files checked
///   VertexDesc (u64) — REPEAT of the header vdesc
///   Triangles (u16[NumTriangles][3]) — DUPLICATE, byte-identical
///   to the struct's triangles; parsed once, duplicate skipped
/// ```
///
/// The vertex-data section is the shape's FULL array, stored ONCE
/// (not per-partition): the body's partition holds 1,385 verts
/// while part 0 lists 1,275. Single-partition shapes have an
/// identity vertex map.
#[derive(Debug, Clone)]
pub struct SkinPartitionBlock {
    pub vertex_desc: u64,
    pub vertices: Vec<BsVertex>,
    pub partitions: Vec<SkinPartitionData>,
    /// Bytes left after parsing (0 in every file checked; non-zero
    /// means a layout variant — the sweep reports it).
    pub trailing_bytes: usize,
}

/// How a `NiTransformData` key group interpolates between its
/// keys. The file stores the type as a u32 per group (0 =
/// constant, 1 = linear, 2 = quadratic); the parser keeps it so
/// the evaluator can interpolate exactly as Gamebryo does —
/// a linear track evaluated as Hermite would ease in and out
/// instead of running straight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyInterpolation {
    /// One value, no time variation.
    Constant,
    /// Straight-line interpolation between keys.
    Linear,
    /// Cubic Hermite interpolation using the keys' tangents.
    Quadratic,
}

impl KeyInterpolation {
    /// The file's u32 key-type code, or `None` for an unknown
    /// code (the caller degrades the block to Unknown).
    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            KEY_CONSTANT => Some(KeyInterpolation::Constant),
            KEY_LINEAR => Some(KeyInterpolation::Linear),
            KEY_QUADRATIC => Some(KeyInterpolation::Quadratic),
            _ => None,
        }
    }
}

/// One float animation key of a `NiTransformData` key group:
/// (time, value, forward tangent, backward tangent). Linear keys
/// carry no tangents (both 0.0); constant keys carry only the
/// value (time 0.0).
///
/// Tangent semantics (proven on the grindstone's `Wheel` data,
/// sawmill milestone): a quadratic tangent is the Hermite
/// tangent PRE-SCALED by the segment duration (m·dt) — the
/// wheel's two keys carry ∓69.115 rad, exactly the segment
/// delta, which makes the Hermite curve exactly linear
/// (constant spin). And the file order is (time, value,
/// BACKWARD, FORWARD): the blade's 93.43 s key reads
/// 1.379 / 0.137, and only with 1.379 as the backward tangent
/// is the slope continuous across the key (0.0477 vs 0.0478
/// rad/s) — the parser reads them in that order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatKey {
    pub time: f32,
    pub value: f32,
    /// Outgoing (forward) tangent, pre-scaled by the segment
    /// duration (m·dt). Drives the segment to the NEXT key.
    pub fwd_tangent: f32,
    /// Incoming (backward) tangent, pre-scaled by the segment
    /// duration (m·dt). Drives the segment from the PREVIOUS
    /// key.
    pub bwd_tangent: f32,
}

/// One vector animation key of a `NiTransformData` translation
/// key group: (time, value, forward tangent, backward tangent),
/// values in Skyrim units. Linear keys carry no tangents (both
/// zero); constant keys carry only the value (time 0.0).
/// Tangents are pre-scaled by the segment duration, as for
/// [`FloatKey`]; the file order is (time, value, backward,
/// forward).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorKey {
    pub time: f32,
    pub value: [f32; 3],
    pub fwd_tangent: [f32; 3],
    pub bwd_tangent: [f32; 3],
}

/// A parsed `NiTransformData` block (M15): the rotation keys of
/// one animation sequence. Only XYZ Euler rotation (type 4) is
/// parsed — every door NIF in Skyrim SE uses it (proven byte-exact
/// on FarmhouseLDoor01 / FarmhouseAnimDoor01 by the `door_anim`
/// probe: 136/136 and 152/152 bytes consumed). Quaternion
/// rotation and other layouts stay `Block::Unknown` until an
/// animation milestone needs them.
///
/// Layout (proven from the bytes, not from nif.xml — the field
/// order there does not match what the files carry):
/// u32 num-rotation-keys (1, a placeholder for XYZ — the real
/// counts live in the per-axis groups), u32 rotation type (4 =
/// XYZ), then THREE float key groups (X, Y, Z), each u32 numKeys,
/// u32 key type (0 constant / 1 linear / 2 quadratic), then the
/// keys; then the translation key group (vec3 values) and the
/// scale key group (float values, skipped — nothing in the
/// engine scales by animation yet). A zero count carries no type
/// field. Translation keys are kept (sawmill sash): doors have
/// none, so the door proofs are unaffected.
#[derive(Debug, Clone)]
pub struct TransformDataBlock {
    /// Per-axis rotation keys, in radians.
    pub x_keys: Vec<FloatKey>,
    pub y_keys: Vec<FloatKey>,
    pub z_keys: Vec<FloatKey>,
    /// Interpolation type of each rotation key group (the
    /// evaluator needs it: linear tracks must not be Hermite'd).
    pub x_key_type: KeyInterpolation,
    pub y_key_type: KeyInterpolation,
    pub z_key_type: KeyInterpolation,
    /// Translation keys, in Skyrim units (NIF-root space).
    /// Empty for doors and for rotation-only data.
    pub translation_keys: Vec<VectorKey>,
    /// Interpolation type of the translation key group
    /// (`Constant` when the group is empty — no type field).
    pub translation_key_type: KeyInterpolation,
}

/// One controlled object of a `NiControllerSequence`: which
/// interpolator drives which node. Only the two fields the door
/// swing needs are kept — the rest of the 29-byte link
/// (controller type, string palette, ...) is skipped at parse
/// time.
#[derive(Debug, Clone)]
pub struct ControlledBlock {
    /// Block index of the `NiTransformInterpolator` that drives
    /// this node in this sequence.
    pub interpolator: i32,
    /// Name of the node this interpolator animates
    /// (`LeftDoor01`, `RightDoor01`, `Door`, ...).
    pub target_name: String,
}

/// A parsed `NiControllerSequence` block (secret door): one
/// animation sequence of a NIF — "Open" and "Close" for doors.
/// The controlled blocks are the game's own answer to WHICH
/// interpolator drives WHICH node, and therefore which open
/// angle each panel of a double door gets. The geometric
/// invariant ("free edge moves toward local +Y") is WRONG for
/// the secret door — both its panels swing toward -Y (into the
/// room) — so the linkage is authoritative, not the geometry.
#[derive(Debug, Clone)]
pub struct ControllerSequenceBlock {
    /// Sequence name ("Open", "Close", ...).
    pub name: String,
    /// Controlled objects, in file order.
    pub controlled: Vec<ControlledBlock>,
}

/// A node the NIF animates through a full continuous turn — a
/// mill waterwheel, a windmill sail: the `NiControllerSequence`
/// drives one local axis through exactly ±2π while the other two
/// axes stay constant. The game loops such a sequence forever
/// (the wheel's is named `Idle`), so the node spins at a constant
/// angular velocity. Door Open/Close sequences never span 2π
/// (the widest is ~101°), so they never match.
#[derive(Debug, Clone, PartialEq)]
pub struct SpinningNode {
    /// Name of the node that spins (`SawWaterWheel03`).
    pub node_name: String,
    /// Which local axis rotates: 0 = X, 1 = Y, 2 = Z.
    pub axis: usize,
    /// Angular velocity in radians per second, signed — the key
    /// delta over the key duration. Negative means the keys run
    /// toward -2π (the wheel's Y keys run 0 -> -2π).
    pub angular_velocity: f32,
    /// Seconds per full turn (2π / |angular_velocity|).
    pub period: f32,
}

impl SpinningNode {
    /// The spinning axis as a unit vector in the node's local
    /// frame.
    pub fn axis_unit(&self) -> [f32; 3] {
        match self.axis {
            0 => [1.0, 0.0, 0.0],
            1 => [0.0, 1.0, 0.0],
            _ => [0.0, 0.0, 1.0],
        }
    }
}

/// A parsed `NiTransformInterpolator` block (secret door): the
/// bridge between a sequence's controlled block and the
/// `NiTransformData` holding the keys. Only the data ref is
/// kept; the transform fields are the "unset" sentinel
/// (-FLT_MAX) when the data drives the transform.
#[derive(Debug, Clone)]
pub struct TransformInterpolatorBlock {
    /// Block index of the `NiTransformData` holding this
    /// interpolator's keys.
    pub data: i32,
}

/// A parsed `NiTransformController` block (sawmill): a keyframed
/// transform animation attached DIRECTLY to a node through its
/// `NiObjectNET` controller ref — not through any sequence. This
/// is how Skyrim animates furniture: the Riverwood sawmill's
/// `SawGear` node carries one (Y keys 0 -> 1080 -> -5 -> 892
/// degrees over 100.73 s), and the grindstone's `Wheel` node
/// carries one (X keys 0 -> -3960 degrees over 9.13 s).
///
/// SE layout (30 bytes, proven byte-exact on 8 controllers in
/// `MillLogPile.nif` and `BlacksmithSharpeningWheelAnimating.nif`
/// via `scan_direct_controllers.py`): i32 next (-1), u16 flags,
/// f32 frequency, f32 phase, f32 start, f32 stop, i32 target
/// (the driven node — always the node that owns the controller),
/// i32 interpolator. Every real controller seen so far carries
/// flags 0x004c, frequency 1.0, phase 0.0 — except the cooking
/// pot's two controllers (0x0048). Hypothesis (UNVERIFIED):
/// bits 1–2 are the cycle type (0 Loop / 1 Reverse / 2 Clamp),
/// which would make the pot loop (an idle sway) and the blade /
/// grindstone clamp (play once per use). The engine currently
/// loops every performance (the provisional trigger); the
/// furniture-interaction milestone should honor the cycle bits
/// once a Reverse example confirms the layout.
#[derive(Debug, Clone)]
pub struct TransformControllerBlock {
    /// Controller flags (0x004c on every real controller seen).
    pub flags: u16,
    /// Playback speed multiplier (1.0 on every real controller).
    pub frequency: f32,
    /// Phase offset in seconds (0.0 on every real controller).
    pub phase: f32,
    /// First key time in seconds.
    pub start_time: f32,
    /// Last key time in seconds.
    pub stop_time: f32,
    /// Block index of the driven node (the controller's owner).
    pub target: i32,
    /// Block index of the `NiTransformInterpolator` that holds
    /// the `NiTransformData` ref.
    pub interpolator: i32,
}

/// One directly-controlled node of a NIF, with its controller
/// timing and key data resolved: the sawmill blade's whole
/// animation in one struct. Returned by
/// `NifFile::direct_controllers()`. Controllers whose chain
/// cannot be resolved (missing interpolator, or data that
/// degraded to Unknown — e.g. quaternion `NiTransformData`,
/// which this parser does not read yet) are skipped, not
/// invented.
#[derive(Debug, Clone)]
pub struct DirectController {
    /// Name of the driven node (`SawGear`, `Wheel`, ...).
    pub node_name: String,
    /// Controller flags (0x004c on every real controller seen).
    pub flags: u16,
    /// Playback speed multiplier (1.0 on every real controller).
    pub frequency: f32,
    /// Phase offset in seconds (0.0 on every real controller).
    pub phase: f32,
    /// First key time in seconds.
    pub start_time: f32,
    /// Last key time in seconds.
    pub stop_time: f32,
    /// The key data: XYZ rotation key groups (radians) +
    /// translation keys (Skyrim units).
    pub data: TransformDataBlock,
}

// ---------------------------------------------------------------------------
// Havok collision blocks (per-object collision milestone).
//
// Every collidable object carries its collision in its NIF as a
// `bhkCollisionObject` -> `bhkRigidBody`/`bhkRigidBodyT` -> shape
// tree. Havok values are in METRES; NIF vertices are in Skyrim
// units (70 per metre), so shape geometry is scaled by
// `HAVOK_TO_SKYRIM` at assembly time (see `collision_shapes`).
//
// Layouts follow the public nif.xml schema as confirmed against
// real files (WalkwayStairs1.nif, FarmhouseLDoor01.nif): the
// collision-object refs (target @0, body @6), the rigid-body
// filter (layer @4, flags @5, response @28, second copy @36-38
// and @48), and the compressed-mesh header (bits 17, winding 18)
// were all verified byte by byte before this parser was written.
// A block that fails to parse degrades to `Block::Unknown` —
// collision is never load-bearing for rendering, and a layout
// this parser does not know must not take the meshes down with
// it (the same rule as the animation blocks).
// ---------------------------------------------------------------------------

/// Havok units (metres) per Skyrim unit: 70 Skyrim units = 1 m.
pub const HAVOK_TO_SKYRIM: f32 = 70.0;

/// Havok collision layers (from nif.xml `SkyrimLayer`).
pub const LAYER_TRIGGER: u8 = 12;
pub const LAYER_NONCOLLIDABLE: u8 = 15;
pub const LAYER_STAIRHELPER: u8 = 31;

/// A 4x4 matrix, COLUMN-major as Havok stores it (four columns of
/// four floats — the opposite convention from [`Mat3`], which is
/// row-major as Gamebryo stores it). Only the collision assembly
/// uses it: node transforms convert in, shape matrices come
/// straight from the file.
pub type Mat4 = [f32; 16];

/// A parsed `bhkCollisionObject`: the link from a scene node to
/// its rigid body. Layout: target node ref @0 (u32), flags u16
/// @4 (unused), body ref @6 (u32). 10 bytes.
#[derive(Debug, Clone)]
pub struct CollisionObjectBlock {
    /// Block index of the target node (the shapes hang under it).
    pub target: i32,
    /// Block index of the `bhkRigidBody`/`bhkRigidBodyT`.
    pub body: i32,
}

/// A parsed `bhkRigidBody` / `bhkRigidBodyT` (Skyrim SE, BS
/// v100). Only the fields collision needs are kept: the shape
/// ref, the layer, whether the body actually collides, and the
/// body's own transform (T variant only). The dynamics
/// (mass/inertia/damping/friction) are not parsed — fixed
/// colliders do not need them; dynamic clutter is a later
/// milestone.
///
/// Layout: shape ref @0 (u32), HavokFilter { layer @4 (u8),
/// flags @5 (u8), group @6 (u16) }, world-object info @8..28,
/// collision response @28 (u8), then `bhkRigidBodyCInfo2010`
/// from @32 with a second HavokFilter { layer @36, flags @37,
/// group @38 } and a second response @48. A body is
/// non-colliding when either filter copy sets flag 0x40 ("No
/// Collision") or either response is 2 (reporting) or 3 (none).
/// `bhkRigidBodyT` additionally carries its transform in
/// otherwise-unused CInfo space: translation (metres) @52,
/// rotation quaternion (x, y, z, w) @68.
#[derive(Debug, Clone)]
pub struct RigidBodyBlock {
    /// Block index of the root shape (`bhkMoppBvTreeShape`,
    /// `bhkListShape`, a primitive, ...).
    pub shape: i32,
    /// Havok layer of the body (1 STATIC, 31 STAIRHELPER, ...).
    pub layer: u8,
    /// False when the filter flags or the collision response say
    /// this body resolves no contacts.
    pub collides: bool,
    /// The body's own transform (T variant only): translation in
    /// Skyrim units, rotation, unit scale. Composes under the
    /// target node's transform.
    pub transform: Option<Transform>,
}

/// A parsed `bhkMoppBvTreeShape`: a MOPP bounding-volume wrapper
/// around one child shape. The MOPP tree itself is never decoded
/// — the child shape IS the geometry.
#[derive(Debug, Clone)]
pub struct MoppShapeBlock {
    /// Block index of the wrapped shape.
    pub child: i32,
}

/// A parsed `bhkTransformShape` / `bhkConvexTransformShape`: one
/// child shape under a 4x4 matrix. Layout: child ref @0 (u32),
/// matrix (16 f32, column-major, translation in metres) @20.
#[derive(Debug, Clone)]
pub struct TransformShapeBlock {
    /// Block index of the wrapped shape.
    pub child: i32,
    /// The shape transform, column-major, translation already in
    /// Skyrim units (x70 at parse time).
    pub matrix: Mat4,
    /// True for `bhkConvexTransformShape`, false for
    /// `bhkTransformShape` (same layout, different type name).
    pub convex: bool,
}

/// A parsed `bhkListShape`: a list of child shapes. Layout:
/// count @0 (u32), child refs @4+4i.
#[derive(Debug, Clone)]
pub struct ListShapeBlock {
    /// Block indices of the child shapes.
    pub children: Vec<i32>,
}

/// A parsed `bhkCompressedMeshShape`: a scale plus a ref to the
/// `bhkCompressedMeshShapeData` holding the triangles. Layout:
/// scale (vec3) @16, data ref @52 (u32).
#[derive(Debug, Clone)]
pub struct CompressedMeshShapeBlock {
    /// Per-axis scale applied to the decoded mesh.
    pub scale: Vec3,
    /// Block index of the `bhkCompressedMeshShapeData`.
    pub data: i32,
}

/// A parsed `bhkCompressedMeshShapeData`: the raw bytes, decoded
/// on demand by the collision assembly (the decode is the
/// expensive part and most NIF parses never need collision).
/// See `decode_compressed_mesh` for the layout.
#[derive(Debug, Clone)]
pub struct CompressedMeshDataBlock {
    /// The block's raw bytes.
    pub bytes: Vec<u8>,
}

/// A parsed `bhkNiTriStripsShape`: a scale plus refs to
/// `NiTriStripsData` blocks (already parsed as geometry data).
/// Layout: scale (vec3) @32, count @48 (u32), data refs @52+4i.
#[derive(Debug, Clone)]
pub struct NiTriStripsShapeBlock {
    /// Per-axis scale applied to the strip geometry.
    pub scale: Vec3,
    /// Block indices of the `NiTriStripsData` blocks.
    pub data_blocks: Vec<i32>,
}

/// A parsed `bhkCapsuleShape`. Layout: endpoint A (vec3,
/// metres) @16, radius @28 (f32), endpoint B (vec3, metres)
/// @32, radius @44 (f32); the radius is max(@28, @44).
#[derive(Debug, Clone)]
pub struct CapsuleShapeBlock {
    /// Endpoint A, metres.
    pub a: Vec3,
    /// Endpoint B, metres.
    pub b: Vec3,
    /// Radius, metres.
    pub radius: f32,
}

/// A parsed `bhkSphereShape`: material @0 (u32, unused), radius
/// (f32, metres) @4. The centre is the shape origin.
#[derive(Debug, Clone)]
pub struct SphereShapeBlock {
    /// Radius, metres.
    pub radius: f32,
}

/// A parsed `bhkCylinderShape`. Layout: vertex A (Vector4,
/// metres) @16, vertex B (Vector4, metres) @32, cylinder radius
/// (f32, metres) @48.
#[derive(Debug, Clone)]
pub struct CylinderShapeBlock {
    /// Axis endpoint A, metres.
    pub a: Vec3,
    /// Axis endpoint B, metres.
    pub b: Vec3,
    /// Radius, metres.
    pub radius: f32,
}

/// A parsed `bhkMultiSphereShape`: 1-8 spheres. Layout: count
/// @16 (u32), then `NiBound` { centre vec3, radius f32 } x n
/// from @20 (16-byte stride, metres).
#[derive(Debug, Clone)]
pub struct MultiSphereShapeBlock {
    /// (centre, radius) pairs, metres.
    pub spheres: Vec<(Vec3, f32)>,
}

/// A parsed `bhkBoxShape`: half-extents (vec3, metres) @16.
#[derive(Debug, Clone)]
pub struct BoxShapeBlock {
    /// Half-extents, metres.
    pub half_extents: Vec3,
}

/// A parsed `bhkConvexVerticesShape`: a convex hull given by its
/// vertices. Layout: count @32 (u32), vertices (Vector4,
/// metres) from @36, 16-byte stride.
#[derive(Debug, Clone)]
pub struct ConvexVerticesShapeBlock {
    /// Hull vertices, metres.
    pub points: Vec<Vec3>,
}

/// One assembled collision shape, in NIF-local Skyrim units with
/// every internal transform baked in (target-node composition,
/// rigid-body-T transform, shape wrappers, shape scale). The
/// scene build converts these to Bevy space exactly like mesh
/// vertices (`to_bevy_position`) and places them with the ref's
/// spawn transform — the same pipeline as the visual geometry,
/// so collision and visuals cannot disagree about placement.
#[derive(Debug, Clone)]
pub enum CollisionShape {
    /// A triangle mesh (compressed mesh / tri-strips data).
    /// Vertices are in NIF-local Skyrim units; triangles index
    /// them. One shape per (body, material) group, so the engine
    /// can tell stair material from wood by `material`.
    Mesh {
        vertices: Vec<Vec3>,
        triangles: Vec<[u32; 3]>,
        /// Havok material code of the triangles (the u32 from the
        /// compressed-mesh material table; 0 when the source
        /// carries none, e.g. tri-strips shapes).
        material: u32,
        /// Havok layer of the owning body (1 STATIC, 31
        /// STAIRHELPER, ...).
        layer: u8,
    },
    /// A capsule (capsule / sphere / multi-sphere shapes): a
    /// segment plus a radius. A sphere is a zero-length capsule
    /// (a == b).
    Capsule {
        a: Vec3,
        b: Vec3,
        radius: f32,
        layer: u8,
    },
    /// A convex hull (box / cylinder / convex-vertices shapes):
    /// the hull's vertices, in NIF-local Skyrim units.
    Hull { points: Vec<Vec3>, layer: u8 },
}

impl CollisionShape {
    /// The owning body's Havok layer.
    pub fn layer(&self) -> u8 {
        match self {
            CollisionShape::Mesh { layer, .. }
            | CollisionShape::Capsule { layer, .. }
            | CollisionShape::Hull { layer, .. } => *layer,
        }
    }
}

/// One parsed block of a NIF file. Unknown block types (animation,
/// collision, particles, ...) are kept as [`Block::Unknown`]
/// with their type name; their bytes were skipped by size.
#[derive(Debug, Clone)]
pub enum Block {
    Node(NodeBlock),
    SwitchNode(SwitchNodeBlock),
    TriShape(TriShapeBlock),
    TriStrips(TriStripsBlock),
    BsLodTriShape(BsLodTriShapeBlock),
    BsTriShape(Box<BsTriShapeBlock>),
    BsDynamicTriShape(Box<BsDynamicTriShapeBlock>),
    TriShapeData(Box<TriShapeDataBlock>),
    TriStripsData(Box<TriStripsDataBlock>),
    TexturingProperty(Box<TexturingPropertyBlock>),
    SourceTexture(SourceTextureBlock),
    LightingShaderProperty(LightingShaderPropertyBlock),
    EffectShaderProperty(EffectShaderPropertyBlock),
    ShaderTextureSet(ShaderTextureSetBlock),
    AlphaProperty(AlphaPropertyBlock),
    MaterialProperty(MaterialPropertyBlock),
    SkinInstance(SkinInstanceBlock),
    SkinData(Box<SkinDataBlock>),
    SkinPartition(Box<SkinPartitionBlock>),
    /// Parsed `NiTransformData` (M15): XYZ rotation keys only.
    TransformData(TransformDataBlock),
    /// Parsed `NiControllerSequence` (secret door): sequence name
    /// + controlled blocks (interpolator + target node).
    ControllerSequence(ControllerSequenceBlock),
    /// Parsed `NiTransformInterpolator` (secret door): the
    /// `NiTransformData` ref.
    TransformInterpolator(TransformInterpolatorBlock),
    /// Parsed `NiTransformController` (sawmill): a keyframed
    /// animation attached directly to a node.
    TransformController(TransformControllerBlock),
    /// Parsed `bhkCollisionObject`: node -> rigid body link.
    CollisionObject(CollisionObjectBlock),
    /// Parsed `bhkRigidBody` / `bhkRigidBodyT`.
    RigidBody(RigidBodyBlock),
    /// Parsed `bhkMoppBvTreeShape`: MOPP wrapper, child is the
    /// geometry.
    MoppShape(MoppShapeBlock),
    /// Parsed `bhkTransformShape` / `bhkConvexTransformShape`.
    TransformShape(TransformShapeBlock),
    /// Parsed `bhkListShape`.
    ListShape(ListShapeBlock),
    /// Parsed `bhkCompressedMeshShape`.
    CompressedMeshShape(CompressedMeshShapeBlock),
    /// Parsed `bhkCompressedMeshShapeData` (raw bytes; decoded
    /// on demand by the collision assembly).
    CompressedMeshShapeData(CompressedMeshDataBlock),
    /// Parsed `bhkNiTriStripsShape`.
    NiTriStripsShape(NiTriStripsShapeBlock),
    /// Parsed `bhkCapsuleShape`.
    CapsuleShape(CapsuleShapeBlock),
    /// Parsed `bhkSphereShape`.
    SphereShape(SphereShapeBlock),
    /// Parsed `bhkCylinderShape`.
    CylinderShape(CylinderShapeBlock),
    /// Parsed `bhkMultiSphereShape`.
    MultiSphereShape(MultiSphereShapeBlock),
    /// Parsed `bhkBoxShape`.
    BoxShape(BoxShapeBlock),
    /// Parsed `bhkConvexVerticesShape`.
    ConvexVerticesShape(ConvexVerticesShapeBlock),
    Unknown {
        type_name: String,
    },
}

impl Block {
    /// The block's type name as stored in the file header.
    pub fn type_name(&self) -> &str {
        match self {
            Block::Node(_) => "NiNode",
            Block::SwitchNode(_) => "NiSwitchNode",
            Block::TriShape(_) => "NiTriShape",
            Block::TriStrips(_) => "NiTriStrips",
            Block::BsLodTriShape(_) => "BSLODTriShape",
            Block::BsTriShape(_) => "BSTriShape",
            Block::BsDynamicTriShape(_) => "BSDynamicTriShape",
            Block::TriShapeData(_) => "NiTriShapeData",
            Block::TriStripsData(_) => "NiTriStripsData",
            Block::TexturingProperty(_) => "NiTexturingProperty",
            Block::SourceTexture(_) => "NiSourceTexture",
            Block::LightingShaderProperty(_) => "BSLightingShaderProperty",
            Block::EffectShaderProperty(_) => "BSEffectShaderProperty",
            Block::ShaderTextureSet(_) => "BSShaderTextureSet",
            Block::AlphaProperty(_) => "NiAlphaProperty",
            Block::MaterialProperty(_) => "NiMaterialProperty",
            Block::SkinInstance(inst) => {
                if inst.dismember {
                    "BSDismemberSkinInstance"
                } else {
                    "NiSkinInstance"
                }
            }
            Block::SkinData(_) => "NiSkinData",
            Block::SkinPartition(_) => "NiSkinPartition",
            Block::TransformData(_) => "NiTransformData",
            Block::ControllerSequence(_) => "NiControllerSequence",
            Block::TransformInterpolator(_) => "NiTransformInterpolator",
            Block::TransformController(_) => "NiTransformController",
            Block::CollisionObject(_) => "bhkCollisionObject",
            // A T body always carries its transform (a bad quat
            // degrades the whole block to Unknown at parse time),
            // so `Some` is exactly the T variant.
            Block::RigidBody(b) => {
                if b.transform.is_some() {
                    "bhkRigidBodyT"
                } else {
                    "bhkRigidBody"
                }
            }
            Block::MoppShape(_) => "bhkMoppBvTreeShape",
            Block::TransformShape(t) => {
                if t.convex {
                    "bhkConvexTransformShape"
                } else {
                    "bhkTransformShape"
                }
            }
            Block::ListShape(_) => "bhkListShape",
            Block::CompressedMeshShape(_) => "bhkCompressedMeshShape",
            Block::CompressedMeshShapeData(_) => "bhkCompressedMeshShapeData",
            Block::NiTriStripsShape(_) => "bhkNiTriStripsShape",
            Block::CapsuleShape(_) => "bhkCapsuleShape",
            Block::SphereShape(_) => "bhkSphereShape",
            Block::CylinderShape(_) => "bhkCylinderShape",
            Block::MultiSphereShape(_) => "bhkMultiSphereShape",
            Block::BoxShape(_) => "bhkBoxShape",
            Block::ConvexVerticesShape(_) => "bhkConvexVerticesShape",
            Block::Unknown { type_name } => type_name,
        }
    }
}

/// The texture paths of a mesh, resolved from its shader's texture
/// set. Paths use backslashes (`textures\...`) and name files inside
/// the game's BSAs.
#[derive(Debug, Clone, Default)]
pub struct Textures {
    pub diffuse: Option<String>,
    pub normal: Option<String>,
    pub glow: Option<String>,
    pub height: Option<String>,
    pub environment: Option<String>,
    pub environment_mask: Option<String>,
    /// The effect shader's second (gradient) texture, when the
    /// shader carries one (e.g. `textures\effects\gradients\
    /// GradCobWeb.dds` on the waterfall fog). The renderer
    /// multiplies it over the diffuse — it is the game's own
    /// variation/falloff for the effect, and without it the fog
    /// sheet reads as a flat slab. `None` for lighting shaders
    /// and for effect shaders without a second string.
    pub gradient: Option<String>,
}

/// A shape's transform (translation, rotation, uniform scale).
/// On [`Mesh`] this is the COMPOSED scene-graph transform: every
/// parent `NiNode` (and `BSFadeNode`, which is a node) composed
/// down to the shape, then the shape's own local transform.
/// M11: the door panel bug was a parent node ignored —
/// `FarmhouseLDoor01.nif` hangs its panel under a node translated
/// (48, 4, 88), so local-only transforms put the panel 1.26 m
/// too low and 48 units sideways.
#[derive(Debug, Clone, Copy)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Mat3,
    pub scale: f32,
}

impl Transform {
    pub fn identity() -> Self {
        Transform {
            translation: [0.0, 0.0, 0.0],
            rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            scale: 1.0,
        }
    }

    /// Compose: `self` is the parent, `child` hangs under it.
    /// Result = self ∘ child: T = T_p + R_p * (s_p * T_c),
    /// R = R_p * R_c, S = s_p * s_c.
    pub fn compose(&self, child: &Transform) -> Transform {
        let scaled = [
            child.translation[0] * self.scale,
            child.translation[1] * self.scale,
            child.translation[2] * self.scale,
        ];
        let rt = mat_vec(&self.rotation, scaled);
        Transform {
            translation: [
                self.translation[0] + rt[0],
                self.translation[1] + rt[1],
                self.translation[2] + rt[2],
            ],
            rotation: mat_mul(&self.rotation, &child.rotation),
            scale: self.scale * child.scale,
        }
    }

    /// Apply the transform to a point: `R * (s * v) + T`.
    pub fn apply(&self, v: Vec3) -> Vec3 {
        let scaled = [v[0] * self.scale, v[1] * self.scale, v[2] * self.scale];
        let rotated = mat_vec(&self.rotation, scaled);
        [
            rotated[0] + self.translation[0],
            rotated[1] + self.translation[1],
            rotated[2] + self.translation[2],
        ]
    }

    /// Apply only the rotation (for normals, tangents, bitangents).
    pub fn rotate(&self, v: Vec3) -> Vec3 {
        mat_vec(&self.rotation, v)
    }
}

/// Row-major 3x3 matrix multiply.
fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}

/// Row-major 3x3 matrix times vector.
fn mat_vec(m: &Mat3, v: Vec3) -> Vec3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

// ---------------------------------------------------------------------------
// Column-major 4x4 matrices (Havok collision only).
//
// Havok stores matrices column-major (four columns of four
// floats) — the opposite of Gamebryo's row-major Mat3. A Mat4 is
// indexed `m[col * 4 + row]`. Composition `mat4_mul(a, b)` is
// "apply b first, then a" (a ∘ b), the same order as
// `Transform::compose` (parent ∘ child).
// ---------------------------------------------------------------------------

/// The identity 4x4 matrix (column-major).
fn mat4_identity() -> Mat4 {
    let mut m = [0.0f32; 16];
    m[0] = 1.0;
    m[5] = 1.0;
    m[10] = 1.0;
    m[15] = 1.0;
    m
}

/// Column-major 4x4 multiply: `a * b` applies b first, then a.
fn mat4_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = a[row] * b[col * 4]
                + a[4 + row] * b[col * 4 + 1]
                + a[8 + row] * b[col * 4 + 2]
                + a[12 + row] * b[col * 4 + 3];
        }
    }
    out
}

/// Apply a column-major 4x4 matrix to a point (w = 1).
fn mat4_point(m: &Mat4, v: Vec3) -> Vec3 {
    [
        m[0] * v[0] + m[4] * v[1] + m[8] * v[2] + m[12],
        m[1] * v[0] + m[5] * v[1] + m[9] * v[2] + m[13],
        m[2] * v[0] + m[6] * v[1] + m[10] * v[2] + m[14],
    ]
}

/// A non-uniform scale as a column-major 4x4 matrix.
fn mat4_scale(s: Vec3) -> Mat4 {
    let mut m = mat4_identity();
    m[0] = s[0];
    m[5] = s[1];
    m[10] = s[2];
    m
}

/// A node [`Transform`] (translation, row-major rotation,
/// uniform scale) as a column-major 4x4 matrix.
fn mat4_from_transform(t: &Transform) -> Mat4 {
    let r = &t.rotation;
    let s = t.scale;
    [
        r[0][0] * s,
        r[1][0] * s,
        r[2][0] * s,
        0.0,
        r[0][1] * s,
        r[1][1] * s,
        r[2][1] * s,
        0.0,
        r[0][2] * s,
        r[1][2] * s,
        r[2][2] * s,
        0.0,
        t.translation[0],
        t.translation[1],
        t.translation[2],
        1.0,
    ]
}

/// The length of the matrix's X column (the scale a capsule
/// radius takes under this transform — exact for uniform scale,
/// the standard approximation otherwise).
fn mat4_x_scale(m: &Mat4) -> f32 {
    (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt()
}

/// A quaternion (x, y, z, w) as a row-major 3x3 rotation matrix
/// — the standard right-handed conversion. The quaternion is
/// normalized first; a zero-length quaternion yields identity.
fn quat_to_mat3(q: [f32; 4]) -> Mat3 {
    let len_sq = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    if len_sq <= 0.0 || !len_sq.is_finite() {
        return Transform::identity().rotation;
    }
    let inv = 1.0 / len_sq.sqrt();
    let (x, y, z, w) = (q[0] * inv, q[1] * inv, q[2] * inv, q[3] * inv);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// Decode a `NiAlphaProperty` into an alpha-test cutoff (0.0–1.0).
/// Bit 9 (0x0200) enables alpha testing; the threshold byte maps
/// to cutoff = threshold / 255. Blend-only properties (bit 0 set
/// without bit 9 — glass, water, ghosts) return `None`: blending
/// is not alpha testing, and the renderer leaves those opaque
/// until a transparent pass exists. The most common foliage flags
/// (0x12ec, 0x1200) are test-only with a GREATER function, which
/// is exactly an alpha-cutout mask.
pub fn alpha_test_cutoff(flags: u16, threshold: u8) -> Option<f32> {
    if flags & 0x0200 != 0 {
        Some(f32::from(threshold) / 255.0)
    } else {
        None
    }
}

/// How a shape's fragments blend with the framebuffer, decoded
/// from its `NiAlphaProperty` flags. Bit 0 enables blending;
/// bits 1–4 are the source mode and bits 5–8 the destination
/// mode (0 ONE, 6 SRC_ALPHA, 7 INV_SRC_ALPHA). A shape can carry
/// BOTH a test (bit 9) and a blend (bit 0); the renderer lets the
/// cutoff win (A2C) and ignores the blend — test+blend shapes are
/// rare and the cutout is the safer read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AlphaBlend {
    /// No blending — the shape is opaque (or alpha-tested).
    #[default]
    Opaque,
    /// Normal alpha blend (src SRC_ALPHA, dst INV_SRC_ALPHA):
    /// fog, glass, water, ghosts. Flags 0x10ed.
    Blend,
    /// Additive blend (src SRC_ALPHA, dst ONE): glows, light
    /// shafts, fire. Flags 0x100d.
    Additive,
}

/// Decode a `NiAlphaProperty`'s blend mode from its flags.
pub fn alpha_blend_mode(flags: u16) -> AlphaBlend {
    if flags & 0x0001 == 0 {
        return AlphaBlend::Opaque;
    }
    let dst = (flags >> 5) & 0x000f;
    if dst == 0 {
        AlphaBlend::Additive
    } else {
        AlphaBlend::Blend
    }
}

/// Plain mesh data, assembled from a shape block and its referenced
/// data, shader, and texture blocks. This is what the renderer will
/// turn into a Bevy mesh.
#[derive(Debug, Clone)]
pub struct Mesh {
    /// The shape's name.
    pub name: String,
    pub vertices: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    pub tangents: Vec<Vec3>,
    pub bitangents: Vec<Vec3>,
    pub vertex_colors: Vec<Color4>,
    /// Triangle indices into the vertex arrays (u32; the file stores
    /// u16).
    pub indices: Vec<u32>,
    /// Material name: the shape's active material (`NiGeometry`), or
    /// the shader property's name (`BSTriShape`). Empty when neither
    /// is set.
    pub material_name: String,
    pub textures: Textures,
    /// Alpha-test cutoff (0.0–1.0) when the shape carries a
    /// `NiAlphaProperty` with alpha testing enabled (bit 9);
    /// `None` otherwise. The renderer maps this to an alpha-cutout
    /// mask so foliage needle cards read as needles, not paper.
    pub alpha_cutoff: Option<f32>,
    /// Blend mode from the shape's `NiAlphaProperty` (bit 0 +
    /// src/dst modes). `Opaque` for shapes with no alpha property
    /// or no blending. The renderer maps `Blend`/`Additive` to
    /// the transparent pass; a shape with BOTH a cutoff and a
    /// blend renders as cutout (cutoff wins).
    pub alpha_blend: AlphaBlend,
    /// True when the shape's shader is a `BSEffectShaderProperty`
    /// (fog, water sheets, glows): the renderer draws it UNLIT —
    /// its color comes from the emissive color and vertex colors,
    /// not the scene lights.
    pub unlit: bool,
    /// Emissive glow from the shape's `BSLightingShaderProperty`
    /// (emissive color × emissive multiple), in linear light.
    /// [0,0,0] when the shape has no emissive shader property or
    /// the color is black. The renderer maps this to
    /// `StandardMaterial.emissive` — the forge coals (white ×
    /// 2.39) glow instead of rendering flat. Effect shaders
    /// carry their emissive in the texture/vertex colors instead
    /// (see `unlit`) — not this field.
    pub emissive: [f32; 3],
    /// Bounding sphere (center, radius) from the geometry data.
    pub bounding_sphere: Option<(Vec3, f32)>,
    /// True when the geometry was reconstructed from the shape's
    /// `NiSkinPartition` in bind pose (M12) rather than read inline.
    pub skinned: bool,
    /// Name of the shape's immediate parent node in the tree walk
    /// (M15). Empty when the shape came from the orphan fallback
    /// or hangs directly under an unnamed root. A door panel's
    /// parent is the hinge node (`Door`, `Door01`, `Door02`, ...).
    pub parent_node: String,
    /// Composed transform of everything ABOVE this shape — the
    /// parent chain only, NOT the shape's own local transform
    /// (that is baked into `transform`). For a door panel under a
    /// hinge node, `parent_transform.translation` IS the hinge
    /// pivot in NIF-root space: the animation rotates the hinge
    /// node about its own origin.
    pub parent_transform: Transform,
    pub transform: Transform,
}

impl Mesh {
    /// Editor-only geometry the retail game never renders:
    /// `EditorMarker` shapes (Creation Kit volume markers with
    /// bright marker vertex colors — the orange salmon-spawn
    /// planes in the river, the 137 m bird-flock volumes in the
    /// sky, the white dust-beam streaks indoors) and particle
    /// emitter volumes (`emitShape*`, `*EmitMesh`, `Emit_*` —
    /// opaque, textureless solids that only define where
    /// particles spawn). v0.4.7 phone feedback: Dylan saw all
    /// of these as yellow shapes, sky poles, and white streaks.
    /// `meshes()` excludes them so every consumer (scene, item
    /// model, doll, NPC) skips them consistently.
    pub fn is_editor_geometry(&self) -> bool {
        is_editor_geometry_name(&self.name, &self.parent_node)
    }
}

/// Name-based editor-geometry test (see `Mesh::
/// is_editor_geometry`). The base name is the part before any
/// `:N` duplicate suffix; the match is case-insensitive because
/// the file's casing is not consistent (`EditorMarker`,
/// `emitShape01`, `splashEmitMesh`, `Emit_LargeOuterArms01`).
pub fn is_editor_geometry_name(name: &str, parent_node: &str) -> bool {
    let base_of = |s: &str| s.split(':').next().unwrap_or(s).to_lowercase();
    let base = base_of(name);
    if base == "editormarker" {
        return true;
    }
    if base.starts_with("emitshape") || base.starts_with("emit_") || base.contains("emitmesh") {
        return true;
    }
    base_of(parent_node) == "editormarker"
}

/// A parsed NIF file: header info, the string table, and the blocks.
#[derive(Debug)]
pub struct NifFile {
    /// NIF version (`0x14020007` for Skyrim).
    pub version: u32,
    /// User version (12 for Skyrim).
    pub user_version: u32,
    /// BS stream version: 83 (Skyrim LE) or 100 (Skyrim SE).
    pub bs_version: u32,
    /// The header string line, without the trailing newline.
    pub header_string: String,
    /// Exporter author string from the BS header.
    pub author: String,
    /// Process script string from the BS header.
    pub process_script: String,
    /// Export script string from the BS header.
    pub export_script: String,
    /// The header string table. Object names and material names are
    /// indices into this table.
    pub strings: Vec<String>,
    /// The file's blocks, in file order. Blocks reference each other
    /// by index into this list.
    pub blocks: Vec<Block>,
    /// Root block indices from the footer (usually block 0).
    pub root_indices: Vec<i32>,
}

/// One sRGB channel (0.0–1.0, higher allowed) to linear light —
/// the standard sRGB EOTF. NIF shader colors are authored in
/// sRGB (the CK color picker); the renderer works in linear.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

impl NifFile {
    /// Parse a NIF file from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    /// Parse a NIF file from its bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);

        // Header string: a line ending in '\n'.
        let header_string = reader.line("header string")?;
        if !header_string.starts_with("Gamebryo File Format, Version ") {
            return Err(NifError::NotANif);
        }

        let version = reader.u32("version")?;
        let endian = reader.u8("endian type")?;
        if endian != 1 {
            return Err(NifError::UnsupportedEndian);
        }
        let user_version = reader.u32("user version")?;
        let num_blocks = reader.u32("num blocks")? as usize;
        if version != NIF_VERSION_SKYRIM || user_version < 3 {
            return Err(NifError::UnsupportedVersion {
                version,
                user_version,
                bs_version: 0,
            });
        }

        // BS stream header (present for 20.2.0.7 with user >= 3).
        let bs_version = reader.u32("BS version")?;
        if bs_version != BS_VERSION_SKYRIM_LE && bs_version != BS_VERSION_SKYRIM_SE {
            return Err(NifError::UnsupportedVersion {
                version,
                user_version,
                bs_version,
            });
        }
        let author = reader.export_string("BS author")?;
        let process_script = if bs_version < 131 {
            reader.export_string("BS process script")?
        } else {
            String::new()
        };
        let export_script = reader.export_string("BS export script")?;
        if bs_version >= 103 {
            // Max Filepath; absent for Skyrim (83/100), read for safety.
            let _max_filepath = reader.export_string("BS max filepath")?;
        }

        let num_block_types = reader.u16("num block types")? as usize;
        let mut block_types = Vec::with_capacity(num_block_types);
        for _ in 0..num_block_types {
            block_types.push(reader.sized_string("block type")?);
        }
        let mut block_type_index = Vec::with_capacity(num_blocks);
        for _ in 0..num_blocks {
            let index = reader.u16("block type index")? as usize;
            if index >= num_block_types {
                return Err(NifError::InvalidData("block type index out of range"));
            }
            block_type_index.push(index);
        }
        let mut block_sizes = Vec::with_capacity(num_blocks);
        for _ in 0..num_blocks {
            block_sizes.push(reader.u32("block size")? as usize);
        }

        let num_strings = reader.u32("num strings")? as usize;
        let _max_string_length = reader.u32("max string length")?;
        let mut strings = Vec::with_capacity(num_strings);
        for _ in 0..num_strings {
            strings.push(reader.sized_string("string table entry")?);
        }
        let num_groups = reader.u32("num groups")? as usize;
        for _ in 0..num_groups {
            let _group = reader.u32("group")?;
        }

        // Blocks, each bounded by its header size.
        let mut blocks = Vec::with_capacity(num_blocks);
        for i in 0..num_blocks {
            let size = block_sizes[i];
            let block_bytes = reader.take(size, "block data")?;
            let type_name = &block_types[block_type_index[i]];
            blocks.push(parse_block(type_name, block_bytes, &strings, bs_version)?);
        }

        // Footer: Num Roots + Roots, when the trailing bytes match
        // exactly. A missing or malformed footer is not fatal.
        let mut root_indices = Vec::new();
        if reader.remaining() >= 4 {
            let mut footer = Reader::new(&reader.bytes[reader.pos..]);
            if let Ok(num_roots) = footer.u32("num roots") {
                let num_roots = num_roots as usize;
                if footer.remaining() == num_roots * 4 {
                    for _ in 0..num_roots {
                        // Length was checked above; unwrap is safe.
                        root_indices.push(footer.i32("root").unwrap());
                    }
                }
            }
        }

        Ok(NifFile {
            version,
            user_version,
            bs_version,
            header_string,
            author,
            process_script,
            export_script,
            strings,
            blocks,
            root_indices,
        })
    }

    /// The block at the given index, or `None` for -1 (null) and
    /// out-of-range indices.
    pub fn block(&self, index: i32) -> Option<&Block> {
        if index < 0 {
            return None;
        }
        self.blocks.get(index as usize)
    }

    /// Every parsed `NiTransformData` block in the file, in file
    /// order (M15). Door NIFs carry two: the Open sequence and the
    /// Close sequence. Blocks whose layout was not XYZ (or that
    /// failed to parse) stayed `Block::Unknown` and are absent.
    pub fn transform_data(&self) -> impl Iterator<Item = &TransformDataBlock> {
        self.blocks.iter().filter_map(|b| match b {
            Block::TransformData(t) => Some(t),
            _ => None,
        })
    }

    /// Per-node open angles from the NIF's "Open" controller
    /// sequence (secret door): node name -> (open angle in
    /// radians, swing duration in seconds). Each controlled block
    /// of the Open sequence names its target node and
    /// interpolator; the interpolator's `NiTransformData` gives
    /// the Z rotation delta (last - first key) and the duration
    /// (last key time). Only links whose Z keys start at the rest
    /// angle (0 modulo 2π) and end elsewhere are included — the
    /// same rest test as `open_angle_from_nif`, so a misnamed
    /// sequence can't produce a bogus angle. Unresolvable links
    /// (missing interpolator, data, or keys) are skipped, not
    /// fatal: the caller falls back to a 1.0 angle scale for
    /// panels with no entry. Empty when the NIF carries no
    /// parseable Open sequence (single-panel doors work off the
    /// door-level angle alone).
    pub fn open_angles_by_node(&self) -> HashMap<String, (f32, f32)> {
        let mut out = HashMap::new();
        let at_rest = |v: f32| {
            let wrapped = v.rem_euclid(2.0 * std::f32::consts::PI);
            wrapped < 1e-3 || wrapped > 2.0 * std::f32::consts::PI - 1e-3
        };
        let Some(seq) = self.blocks.iter().find_map(|b| match b {
            Block::ControllerSequence(s) if s.name == "Open" => Some(s),
            _ => None,
        }) else {
            return out;
        };
        for link in &seq.controlled {
            let Some(Block::TransformInterpolator(interp)) = self.block(link.interpolator) else {
                continue;
            };
            let Some(Block::TransformData(data)) = self.block(interp.data) else {
                continue;
            };
            let (Some(first), Some(last)) = (data.z_keys.first(), data.z_keys.last()) else {
                continue;
            };
            if !at_rest(first.value) || at_rest(last.value) {
                continue;
            }
            out.insert(
                link.target_name.clone(),
                (last.value - first.value, last.time),
            );
        }
        out
    }

    /// Nodes this NIF spins through a full continuous turn (mill
    /// wheel): every controlled link, in every sequence, whose
    /// `NiTransformData` drives exactly one axis through ±2π
    /// while the other two stay constant. The angular velocity
    /// is the key delta over the key duration; the period is one
    /// full turn. Links that do not resolve (missing interpolator
    /// or data, too few keys, zero duration) are skipped, not
    /// fatal. A node driven by two sequences appears once (the
    /// first link wins). Empty for NIFs with no spinning node —
    /// doors included (their widest swing is ~101°).
    pub fn spinning_nodes(&self) -> Vec<SpinningNode> {
        let mut out: Vec<SpinningNode> = Vec::new();
        let two_pi = 2.0 * std::f32::consts::PI;
        for block in &self.blocks {
            let Block::ControllerSequence(seq) = block else {
                continue;
            };
            for link in &seq.controlled {
                if out.iter().any(|s| s.node_name == link.target_name) {
                    continue;
                }
                let Some(Block::TransformInterpolator(interp)) = self.block(link.interpolator)
                else {
                    continue;
                };
                let Some(Block::TransformData(data)) = self.block(interp.data) else {
                    continue;
                };
                let axes = [&data.x_keys, &data.y_keys, &data.z_keys];
                for (axis, keys) in axes.iter().enumerate() {
                    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
                        continue;
                    };
                    if keys.len() < 2 || last.time <= first.time {
                        continue;
                    }
                    let delta = last.value - first.value;
                    if (delta.abs() - two_pi).abs() > 1e-3 {
                        continue;
                    }
                    // The other two axes must be constant.
                    let mut constant = true;
                    for (other, okeys) in axes.iter().enumerate() {
                        if other == axis {
                            continue;
                        }
                        if let (Some(of), Some(ol)) = (okeys.first(), okeys.last()) {
                            if (ol.value - of.value).abs() > 1e-3 {
                                constant = false;
                            }
                        }
                    }
                    if !constant {
                        continue;
                    }
                    let duration = last.time - first.time;
                    let angular_velocity = delta / duration;
                    out.push(SpinningNode {
                        node_name: link.target_name.clone(),
                        axis,
                        angular_velocity,
                        period: two_pi / angular_velocity.abs(),
                    });
                }
            }
        }
        out
    }

    /// Nodes this NIF animates through a DIRECT `NiTransformController`
    /// (sawmill): every `NiNode` whose `NiObjectNET` controller ref
    /// points at a parsed controller whose interpolator resolves to
    /// parsed `NiTransformData`. This is the furniture pattern —
    /// the sawmill's `SawGear` blade and the grindstone's `Wheel`
    /// — and it is disjoint from `spinning_nodes()`, which only
    /// scans sequences. Links that do not resolve (missing
    /// interpolator or data, or data that degraded to Unknown —
    /// e.g. quaternion `NiTransformData`, which this parser does
    /// not read yet) are skipped, not fatal. `NiSwitchNode`
    /// blocks are not scanned: no real switch node carries a
    /// direct controller.
    pub fn direct_controllers(&self) -> Vec<DirectController> {
        let mut out: Vec<DirectController> = Vec::new();
        for block in &self.blocks {
            let Block::Node(node) = block else {
                continue;
            };
            let controller_ref = node.av.net.controller;
            if controller_ref < 0 {
                continue;
            }
            let Some(Block::TransformController(ctl)) = self.block(controller_ref) else {
                continue;
            };
            let Some(Block::TransformInterpolator(interp)) = self.block(ctl.interpolator) else {
                continue;
            };
            let Some(Block::TransformData(data)) = self.block(interp.data) else {
                continue;
            };
            out.push(DirectController {
                node_name: node.av.net.name.clone(),
                flags: ctl.flags,
                frequency: ctl.frequency,
                phase: ctl.phase,
                start_time: ctl.start_time,
                stop_time: ctl.stop_time,
                data: data.clone(),
            });
        }
        out
    }

    /// Assemble plain mesh data from every shape block in the file:
    /// `NiTriShape`, `NiTriStrips`, `BSLODTriShape`, `BSTriShape`,
    /// and `BSDynamicTriShape`. Only drawable meshes are returned:
    /// shapes whose data block is missing or unresolvable are
    /// skipped, and so are shapes with no vertices or no indices.
    ///
    /// Skinned SSE shapes (M12) store their geometry in the shape's
    /// `NiSkinPartition`, not the shape block: those are reconstructed
    /// in bind pose — per vertex, `v' = Σ_b w_b × (BoneWorld_b ∘
    /// BoneSkin_b ∘ GeomSkin) × v`, where `BoneWorld` is the bone
    /// node's composed scene-graph transform, `BoneSkin` is the
    /// `NiSkinData` bone transform, and `GeomSkin` is the
    /// `NiSkinData` geometry transform. The shape's own composed
    /// transform is NOT baked in: it stays on `Mesh.transform`, and
    /// the renderer applies it exactly once (linearity makes the two
    /// identical). Normals, tangents, and bitangents get the same
    /// weighted treatment, rotation-only, renormalized — required
    /// because thigh/calf bind rotations are ~180° about Y and flip
    /// normals. The bind-pose formula is numerically proven on
    /// `MaleBody_1.nif` and the FaceGen heads (see
    /// `docs/formats/nif.md`); LE-style skinning (`NiTriShape` +
    /// `NiSkinInstance`) does not occur in Skyrim SE's Meshes BSAs
    /// and is left as-is (inline vertices, no skinning).
    ///
    /// Each mesh's transform is the COMPOSED scene-graph transform:
    /// the tree is walked from the footer roots, composing every
    /// parent node's transform down to the shape. Shapes not
    /// reachable from any root (orphans, or files with no usable
    /// roots) fall back to their local transform, so nothing that
    /// used to assemble is ever lost. Meshes come back in block
    /// order, as before. Editor-only geometry is excluded (see
    /// `Mesh::is_editor_geometry`): `EditorMarker` shapes and
    /// particle emitter volumes never render in the retail game.
    pub fn meshes(&self) -> Vec<Mesh> {
        let mut found: Vec<(i32, Mesh)> = Vec::new();
        let mut visited: HashSet<i32> = HashSet::new();
        // Composed world transform of every node the walk reaches:
        // the skinning reconstruction needs each bone node's world
        // transform. Bones the walk never reaches fall back to
        // their local transform inside `mesh_from_skin`.
        let mut node_worlds: HashMap<i32, Transform> = HashMap::new();
        let roots: Vec<i32> = if self.root_indices.is_empty() {
            vec![0]
        } else {
            self.root_indices.clone()
        };
        // Stack: (block index, composed transform of everything
        // ABOVE this block, name of the immediate parent node).
        // A node's own transform composes under its parent's
        // when descending to its children. The parent name rides
        // along so meshes know their hinge node (M15).
        let mut stack: Vec<(i32, Transform, String)> = roots
            .into_iter()
            .map(|r| (r, Transform::identity(), String::new()))
            .collect();
        while let Some((idx, parent, parent_name)) = stack.pop() {
            if !visited.insert(idx) {
                continue;
            }
            match self.block(idx) {
                Some(Block::Node(node)) => {
                    let composed = parent.compose(&Transform {
                        translation: node.av.translation,
                        rotation: node.av.rotation,
                        scale: node.av.scale,
                    });
                    node_worlds.insert(idx, composed);
                    let name = node.av.net.name.clone();
                    for child in &node.children {
                        stack.push((*child, composed, name.clone()));
                    }
                }
                Some(Block::SwitchNode(switch)) => {
                    // M13: only the ACTIVE child renders. The
                    // stored index selects it (child 0 = the
                    // full-detail tree for pines); an out-of-range
                    // index falls back to child 0. Inactive
                    // subtrees are marked visited WITHOUT
                    // collecting meshes, so the fallback loop
                    // below cannot resurrect them — without this
                    // the LOD tree would still render via the
                    // fallback and the pines would stay doubled.
                    let composed = parent.compose(&Transform {
                        translation: switch.node.av.translation,
                        rotation: switch.node.av.rotation,
                        scale: switch.node.av.scale,
                    });
                    node_worlds.insert(idx, composed);
                    let n = switch.node.children.len();
                    let active = if switch.active_child >= 0 && (switch.active_child as usize) < n {
                        switch.active_child as usize
                    } else {
                        0
                    };
                    let name = switch.node.av.net.name.clone();
                    for (i, child) in switch.node.children.iter().enumerate() {
                        if i == active {
                            stack.push((*child, composed, name.clone()));
                        } else {
                            self.mark_subtree(*child, &mut visited);
                        }
                    }
                }
                Some(block) => {
                    if let Some(mesh) =
                        self.mesh_from_block(block, &parent, &parent_name, &node_worlds)
                    {
                        if !mesh.vertices.is_empty()
                            && !mesh.indices.is_empty()
                            && !mesh.is_editor_geometry()
                        {
                            found.push((idx, mesh));
                        }
                    }
                }
                None => {}
            }
        }
        // Fallback: shapes the tree walk never reached keep their
        // local transform (the pre-M11 behavior).
        for (i, block) in self.blocks.iter().enumerate() {
            let idx = i as i32;
            if visited.contains(&idx) {
                continue;
            }
            if let Some(mesh) =
                self.mesh_from_block(block, &Transform::identity(), "", &node_worlds)
            {
                if !mesh.vertices.is_empty()
                    && !mesh.indices.is_empty()
                    && !mesh.is_editor_geometry()
                {
                    found.push((idx, mesh));
                }
            }
        }
        found.sort_by_key(|(idx, _)| *idx);
        found.into_iter().map(|(_, mesh)| mesh).collect()
    }

    /// Mark a subtree as visited WITHOUT collecting meshes (for
    /// inactive switch children). The fallback in `meshes()`
    /// skips visited blocks, so suppressed shapes never render.
    /// NIF children form a tree in practice, so a block marked
    /// here is not reachable from any active path.
    fn mark_subtree(&self, root: i32, visited: &mut HashSet<i32>) {
        let mut stack = vec![root];
        while let Some(idx) = stack.pop() {
            if !visited.insert(idx) {
                continue;
            }
            match self.block(idx) {
                Some(Block::Node(node)) => {
                    for child in &node.children {
                        stack.push(*child);
                    }
                }
                Some(Block::SwitchNode(switch)) => {
                    for child in &switch.node.children {
                        stack.push(*child);
                    }
                }
                _ => {}
            }
        }
    }

    /// Assemble the authored Havok collision shapes of the file:
    /// one [`CollisionShape`] per (body, shape) — meshes split
    /// further per material, so the engine can tell stair
    /// material from wood. All points are in NIF-local Skyrim
    /// units with every internal transform baked in (target-node
    /// composition, rigid-body-T transform, shape wrappers,
    /// shape scale); the scene build converts them to Bevy space
    /// exactly like mesh vertices.
    ///
    /// Bodies are skipped when their layer is TRIGGER (12) or
    /// NONCOLLIDABLE (15), when their filter flags or collision
    /// response say they resolve no contacts, or when their
    /// shape tree cannot be followed. A shape that fails to
    /// decode (a compressed mesh with an unsupported layout, a
    /// dangling ref) is skipped, not fatal — the rest of the
    /// file's collision still assembles, exactly like
    /// [`NifFile::meshes`] skips unresolvable shapes.
    pub fn collision_shapes(&self) -> Vec<CollisionShape> {
        // Parent map + local transforms, from every node block.
        let mut parent: HashMap<i32, i32> = HashMap::new();
        let mut local: HashMap<i32, Transform> = HashMap::new();
        for (i, block) in self.blocks.iter().enumerate() {
            let idx = i as i32;
            let (node, children): (&NodeBlock, &[i32]) = match block {
                Block::Node(node) => (node, &node.children),
                Block::SwitchNode(switch) => (&switch.node, &switch.node.children),
                _ => continue,
            };
            local.insert(
                idx,
                Transform {
                    translation: node.av.translation,
                    rotation: node.av.rotation,
                    scale: node.av.scale,
                },
            );
            for child in children {
                parent.insert(*child, idx);
            }
        }
        let mut out = Vec::new();
        for block in &self.blocks {
            let Block::CollisionObject(obj) = block else {
                continue;
            };
            let Some(Block::RigidBody(body)) = self.block(obj.body) else {
                continue;
            };
            if body.layer == LAYER_TRIGGER || body.layer == LAYER_NONCOLLIDABLE {
                continue;
            }
            if !body.collides {
                continue;
            }
            let mut frame = self.node_frame(obj.target, &parent, &local);
            if let Some(t) = &body.transform {
                frame = mat4_mul(&frame, &mat4_from_transform(t));
            }
            self.extract_collision_shape(body.shape, &frame, body.layer, &mut out, 0);
            if out.len() >= MAX_COLLISION_SHAPES {
                break;
            }
        }
        out
    }

    /// The composed world transform of a node, as a column-major
    /// 4x4 matrix: the product of every ancestor's local
    /// transform from the root down to the node. A node with no
    /// ancestors (or a non-node target) gets identity — the same
    /// fallback `meshes()` uses for unreachable shapes. The
    /// parent chain is cycle-guarded.
    fn node_frame(
        &self,
        target: i32,
        parent: &HashMap<i32, i32>,
        local: &HashMap<i32, Transform>,
    ) -> Mat4 {
        let mut chain: Vec<Transform> = Vec::new();
        let mut seen: HashSet<i32> = HashSet::new();
        let mut idx = target;
        while seen.insert(idx) {
            let Some(t) = local.get(&idx) else {
                break;
            };
            chain.push(Transform {
                translation: t.translation,
                rotation: t.rotation,
                scale: t.scale,
            });
            let Some(p) = parent.get(&idx) else {
                break;
            };
            idx = *p;
        }
        // Chain is target-up; compose root-down.
        let mut frame = Transform::identity();
        for t in chain.iter().rev() {
            frame = frame.compose(t);
        }
        mat4_from_transform(&frame)
    }

    /// Extract one shape (and its children, for wrappers) into
    /// `out`, under the accumulated `frame`. Depth- and
    /// size-capped; unresolvable or unsupported shapes are
    /// skipped, not fatal.
    fn extract_collision_shape(
        &self,
        index: i32,
        frame: &Mat4,
        layer: u8,
        out: &mut Vec<CollisionShape>,
        depth: usize,
    ) {
        if depth >= 16 || out.len() >= MAX_COLLISION_SHAPES {
            return;
        }
        let Some(block) = self.block(index) else {
            return;
        };
        match block {
            Block::MoppShape(mopp) => {
                self.extract_collision_shape(mopp.child, frame, layer, out, depth + 1);
            }
            Block::TransformShape(t) => {
                let next = mat4_mul(frame, &t.matrix);
                self.extract_collision_shape(t.child, &next, layer, out, depth + 1);
            }
            Block::ListShape(list) => {
                for child in &list.children {
                    self.extract_collision_shape(*child, frame, layer, out, depth + 1);
                    if out.len() >= MAX_COLLISION_SHAPES {
                        return;
                    }
                }
            }
            Block::CompressedMeshShape(shape) => {
                let Some(Block::CompressedMeshShapeData(data)) = self.block(shape.data) else {
                    return;
                };
                let Some(decoded) = decode_compressed_mesh(&data.bytes) else {
                    return;
                };
                let frame = mat4_mul(frame, &mat4_scale(shape.scale));
                let vertices: Vec<Vec3> = decoded
                    .vertices
                    .iter()
                    .map(|v| mat4_point(&frame, *v))
                    .collect();
                if vertices.len() > MAX_COLLISION_VERTICES {
                    return;
                }
                for (material, _mat_layer, triangles) in decoded.groups {
                    if triangles.is_empty() {
                        continue;
                    }
                    out.push(CollisionShape::Mesh {
                        vertices: vertices.clone(),
                        triangles,
                        material,
                        layer,
                    });
                    if out.len() >= MAX_COLLISION_SHAPES {
                        return;
                    }
                }
            }
            Block::NiTriStripsShape(shape) => {
                let frame = mat4_mul(frame, &mat4_scale(shape.scale));
                for data_index in &shape.data_blocks {
                    let Some(Block::TriStripsData(strips)) = self.block(*data_index) else {
                        continue;
                    };
                    let vertices: Vec<Vec3> = strips
                        .data
                        .vertices
                        .iter()
                        .map(|v| mat4_point(&frame, *v))
                        .collect();
                    if vertices.is_empty() || vertices.len() > MAX_COLLISION_VERTICES {
                        continue;
                    }
                    let mut triangles: Vec<[u32; 3]> = Vec::new();
                    for strip in &strips.strips {
                        for tri in triangulate_strip(strip) {
                            let tri = [u32::from(tri[0]), u32::from(tri[1]), u32::from(tri[2])];
                            if tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2] {
                                triangles.push(tri);
                            }
                        }
                    }
                    if triangles.is_empty() {
                        continue;
                    }
                    out.push(CollisionShape::Mesh {
                        vertices,
                        triangles,
                        material: 0,
                        layer,
                    });
                    if out.len() >= MAX_COLLISION_SHAPES {
                        return;
                    }
                }
            }
            Block::CapsuleShape(capsule) => {
                let scale = mat4_x_scale(frame);
                let radius = capsule.radius * HAVOK_TO_SKYRIM * scale;
                if !radius.is_finite() || radius <= 0.0 {
                    return;
                }
                let a = mat4_point(
                    frame,
                    [
                        capsule.a[0] * HAVOK_TO_SKYRIM,
                        capsule.a[1] * HAVOK_TO_SKYRIM,
                        capsule.a[2] * HAVOK_TO_SKYRIM,
                    ],
                );
                let b = mat4_point(
                    frame,
                    [
                        capsule.b[0] * HAVOK_TO_SKYRIM,
                        capsule.b[1] * HAVOK_TO_SKYRIM,
                        capsule.b[2] * HAVOK_TO_SKYRIM,
                    ],
                );
                out.push(CollisionShape::Capsule {
                    a,
                    b,
                    radius,
                    layer,
                });
            }
            Block::SphereShape(sphere) => {
                let scale = mat4_x_scale(frame);
                let radius = sphere.radius * HAVOK_TO_SKYRIM * scale;
                if !radius.is_finite() || radius <= 0.0 {
                    return;
                }
                let center = mat4_point(frame, [0.0, 0.0, 0.0]);
                out.push(CollisionShape::Capsule {
                    a: center,
                    b: center,
                    radius,
                    layer,
                });
            }
            Block::CylinderShape(cylinder) => {
                // A 16-point x 2-ring hull around the axis.
                let a = [
                    cylinder.a[0] * HAVOK_TO_SKYRIM,
                    cylinder.a[1] * HAVOK_TO_SKYRIM,
                    cylinder.a[2] * HAVOK_TO_SKYRIM,
                ];
                let b = [
                    cylinder.b[0] * HAVOK_TO_SKYRIM,
                    cylinder.b[1] * HAVOK_TO_SKYRIM,
                    cylinder.b[2] * HAVOK_TO_SKYRIM,
                ];
                let radius = cylinder.radius * HAVOK_TO_SKYRIM;
                let axis = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
                if !len.is_finite() || len < 1e-6 {
                    return;
                }
                let axis = [axis[0] / len, axis[1] / len, axis[2] / len];
                let helper: Vec3 = if axis[0].abs() < 0.9 {
                    [1.0, 0.0, 0.0]
                } else {
                    [0.0, 1.0, 0.0]
                };
                let dot = helper[0] * axis[0] + helper[1] * axis[1] + helper[2] * axis[2];
                let mut u = [
                    helper[0] - axis[0] * dot,
                    helper[1] - axis[1] * dot,
                    helper[2] - axis[2] * dot,
                ];
                let ulen = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
                if !ulen.is_finite() || ulen < 1e-6 {
                    return;
                }
                u = [u[0] / ulen, u[1] / ulen, u[2] / ulen];
                let v = [
                    axis[1] * u[2] - axis[2] * u[1],
                    axis[2] * u[0] - axis[0] * u[2],
                    axis[0] * u[1] - axis[1] * u[0],
                ];
                let mut points = Vec::with_capacity(32);
                for end in [a, b] {
                    for i in 0..16 {
                        let angle = i as f32 / 16.0 * std::f32::consts::TAU;
                        let (sin, cos) = angle.sin_cos();
                        let p = [
                            end[0] + (u[0] * cos + v[0] * sin) * radius,
                            end[1] + (u[1] * cos + v[1] * sin) * radius,
                            end[2] + (u[2] * cos + v[2] * sin) * radius,
                        ];
                        points.push(mat4_point(frame, p));
                    }
                }
                out.push(CollisionShape::Hull { points, layer });
            }
            Block::MultiSphereShape(multi) => {
                let scale = mat4_x_scale(frame);
                for (center, radius) in &multi.spheres {
                    let radius = radius * HAVOK_TO_SKYRIM * scale;
                    if !radius.is_finite() || radius <= 0.0 {
                        continue;
                    }
                    let center = mat4_point(
                        frame,
                        [
                            center[0] * HAVOK_TO_SKYRIM,
                            center[1] * HAVOK_TO_SKYRIM,
                            center[2] * HAVOK_TO_SKYRIM,
                        ],
                    );
                    out.push(CollisionShape::Capsule {
                        a: center,
                        b: center,
                        radius,
                        layer,
                    });
                    if out.len() >= MAX_COLLISION_SHAPES {
                        return;
                    }
                }
            }
            Block::BoxShape(box_shape) => {
                let half = [
                    box_shape.half_extents[0] * HAVOK_TO_SKYRIM,
                    box_shape.half_extents[1] * HAVOK_TO_SKYRIM,
                    box_shape.half_extents[2] * HAVOK_TO_SKYRIM,
                ];
                let mut points = Vec::with_capacity(8);
                for x in [-1.0, 1.0] {
                    for y in [-1.0, 1.0] {
                        for z in [-1.0, 1.0] {
                            points.push(mat4_point(frame, [half[0] * x, half[1] * y, half[2] * z]));
                        }
                    }
                }
                out.push(CollisionShape::Hull { points, layer });
            }
            Block::ConvexVerticesShape(convex) => {
                let points: Vec<Vec3> = convex
                    .points
                    .iter()
                    .map(|p| {
                        mat4_point(
                            frame,
                            [
                                p[0] * HAVOK_TO_SKYRIM,
                                p[1] * HAVOK_TO_SKYRIM,
                                p[2] * HAVOK_TO_SKYRIM,
                            ],
                        )
                    })
                    .collect();
                if points.len() < 4 {
                    return;
                }
                out.push(CollisionShape::Hull { points, layer });
            }
            _ => {}
        }
    }

    /// Build a mesh from one shape block under a composed parent
    /// transform, or `None` when the block is not a drawable shape.
    /// `node_worlds` maps node block indices to their composed
    /// world transforms, for the skinning reconstruction.
    /// `parent_name` is the immediate parent node's name (M15).
    fn mesh_from_block(
        &self,
        block: &Block,
        parent: &Transform,
        parent_name: &str,
        node_worlds: &HashMap<i32, Transform>,
    ) -> Option<Mesh> {
        match block {
            Block::TriShape(shape) => self.mesh_from_geometry(&shape.geometry, parent, parent_name),
            Block::TriStrips(shape) => {
                self.mesh_from_geometry(&shape.geometry, parent, parent_name)
            }
            Block::BsLodTriShape(shape) => {
                self.mesh_from_geometry(&shape.geometry, parent, parent_name)
            }
            Block::BsTriShape(shape) => {
                Some(self.mesh_from_bs_tri_shape(shape, parent, parent_name, node_worlds, None))
            }
            Block::BsDynamicTriShape(dynamic) => Some(self.mesh_from_bs_tri_shape(
                &dynamic.shape,
                parent,
                parent_name,
                node_worlds,
                Some(&dynamic.dynamic_positions),
            )),
            _ => None,
        }
    }

    /// Resolve a shape's `NiAlphaProperty` index to an alpha-test
    /// cutoff, or `None` when there is no property (-1), it does
    /// not resolve, or it is blend-only.
    fn alpha_cutoff(&self, alpha_index: i32) -> Option<f32> {
        let Block::AlphaProperty(prop) = self.block(alpha_index)? else {
            return None;
        };
        alpha_test_cutoff(prop.flags, prop.threshold)
    }

    /// Resolve a shape's `NiAlphaProperty` index to a blend mode,
    /// or `Opaque` when there is no property, it does not resolve,
    /// or blending is not enabled.
    fn alpha_blend(&self, alpha_index: i32) -> AlphaBlend {
        let Some(Block::AlphaProperty(prop)) = self.block(alpha_index) else {
            return AlphaBlend::Opaque;
        };
        alpha_blend_mode(prop.flags)
    }

    /// True when the shape's shader property is a
    /// `BSEffectShaderProperty` (fog, water, glows).
    fn is_effect_shader(&self, shader_index: i32) -> bool {
        matches!(
            self.block(shader_index),
            Some(Block::EffectShaderProperty(_))
        )
    }

    /// The texture paths for a shape's shader property, resolved
    /// through its `BSShaderTextureSet`. Missing links yield `None`
    /// slots rather than an error. An effect shader has no texture
    /// set — its single source texture IS the diffuse.
    fn shader_textures(&self, shader_index: i32) -> Textures {
        let mut textures = Textures::default();
        match self.block(shader_index) {
            Some(Block::EffectShaderProperty(shader)) => {
                if !shader.source_texture.is_empty() {
                    textures.diffuse = Some(shader.source_texture.clone());
                }
                textures.gradient = shader.gradient_texture.clone();
                textures
            }
            Some(Block::LightingShaderProperty(shader)) => {
                let Some(Block::ShaderTextureSet(set)) = self.block(shader.texture_set) else {
                    return textures;
                };
                let get = |slot: usize| set.textures.get(slot).filter(|s| !s.is_empty()).cloned();
                textures.diffuse = get(0);
                textures.normal = get(1);
                textures.glow = get(2);
                textures.height = get(3);
                textures.environment = get(4);
                textures.environment_mask = get(5);
                textures
            }
            _ => textures,
        }
    }

    /// The name of a shader property block, or "" when unresolvable.
    fn shader_name(&self, shader_index: i32) -> String {
        match self.block(shader_index) {
            Some(Block::LightingShaderProperty(shader)) => shader.net.name.clone(),
            Some(Block::EffectShaderProperty(shader)) => shader.net.name.clone(),
            _ => String::new(),
        }
    }

    /// Resolve a shape's shader property to its emissive glow:
    /// the `BSLightingShaderProperty` emissive color (sRGB in
    /// the file, converted to linear — the same convention as
    /// VCLR) times the emissive multiple. [0,0,0] when the
    /// property is missing, not a lighting shader, or black.
    /// Effect shaders are not resolved here (see `Mesh::unlit`).
    fn shader_emissive(&self, shader_index: i32) -> [f32; 3] {
        let Some(Block::LightingShaderProperty(prop)) = self.block(shader_index) else {
            return [0.0, 0.0, 0.0];
        };
        let c = prop.emissive_color;
        let m = prop.emissive_multiple;
        if m <= 0.0 || (c[0] <= 0.0 && c[1] <= 0.0 && c[2] <= 0.0) {
            return [0.0, 0.0, 0.0];
        }
        [
            srgb_to_linear(c[0]) * m,
            srgb_to_linear(c[1]) * m,
            srgb_to_linear(c[2]) * m,
        ]
    }

    fn mesh_from_geometry(
        &self,
        geometry: &Geometry,
        parent: &Transform,
        parent_name: &str,
    ) -> Option<Mesh> {
        let (data, indices) = match self.block(geometry.data) {
            Some(Block::TriShapeData(tri)) => (
                &tri.data,
                tri.triangles
                    .iter()
                    .flat_map(|t| t.iter().map(|v| u32::from(*v)))
                    .collect::<Vec<u32>>(),
            ),
            Some(Block::TriStripsData(strips)) => {
                let mut indices = Vec::new();
                for strip in &strips.strips {
                    for tri in triangulate_strip(strip) {
                        indices.extend(tri.iter().map(|v| u32::from(*v)));
                    }
                }
                (&strips.data, indices)
            }
            _ => return None,
        };
        let material_name =
            material_name_for(geometry, &self.shader_name(geometry.shader_property));
        let local = Transform {
            translation: geometry.av.translation,
            rotation: geometry.av.rotation,
            scale: geometry.av.scale,
        };
        Some(Mesh {
            name: geometry.av.net.name.clone(),
            vertices: data.vertices.clone(),
            normals: data.normals.clone(),
            uvs: data.uvs.clone(),
            tangents: data.tangents.clone(),
            bitangents: data.bitangents.clone(),
            vertex_colors: data.vertex_colors.clone(),
            indices,
            material_name,
            textures: self.shader_textures(geometry.shader_property),
            alpha_cutoff: self.alpha_cutoff(geometry.alpha_property),
            alpha_blend: self.alpha_blend(geometry.alpha_property),
            unlit: self.is_effect_shader(geometry.shader_property),
            emissive: self.shader_emissive(geometry.shader_property),
            bounding_sphere: Some(data.bounding_sphere),
            skinned: false,
            parent_node: parent_name.to_string(),
            parent_transform: *parent,
            transform: parent.compose(&local),
        })
    }

    fn mesh_from_bs_tri_shape(
        &self,
        shape: &BsTriShapeBlock,
        parent: &Transform,
        parent_name: &str,
        node_worlds: &HashMap<i32, Transform>,
        dynamic_positions: Option<&[Vec3]>,
    ) -> Mesh {
        // Skinned shape with no inline vertices: reconstruct from
        // the skin partition. `dynamic_positions` is `Some` for a
        // `BSDynamicTriShape` (positions come ONLY from the dynamic
        // section — the partition's vertex desc has no position bit)
        // and `None` for a `BSTriShape` (positions from the
        // partition). When the skin links don't resolve, fall
        // through to the (empty) inline mesh — the old behavior,
        // skipped by `meshes()`. A shape WITH inline vertices keeps
        // them even if it also has a skin instance.
        if shape.vertices.is_empty() && shape.skin >= 0 {
            if let Some(mesh) =
                self.mesh_from_skin(shape, dynamic_positions, parent, parent_name, node_worlds)
            {
                return mesh;
            }
        }
        let vertices: Vec<Vec3> = shape
            .vertices
            .iter()
            .map(|v| v.position.unwrap_or([0.0, 0.0, 0.0]))
            .collect();
        let attrs = bs_attributes(&shape.vertices);
        let normals = attrs.normals;
        let uvs = attrs.uvs;
        let tangents = attrs.tangents;
        let bitangents = attrs.bitangents;
        let vertex_colors = attrs.vertex_colors;
        let indices = shape
            .triangles
            .iter()
            .flat_map(|t| t.iter().map(|v| u32::from(*v)))
            .collect();
        let local = Transform {
            translation: shape.av.translation,
            rotation: shape.av.rotation,
            scale: shape.av.scale,
        };
        Mesh {
            name: shape.av.net.name.clone(),
            vertices,
            normals,
            uvs,
            tangents,
            bitangents,
            vertex_colors,
            indices,
            material_name: self.shader_name(shape.shader_property),
            textures: self.shader_textures(shape.shader_property),
            alpha_cutoff: self.alpha_cutoff(shape.alpha_property),
            alpha_blend: self.alpha_blend(shape.alpha_property),
            unlit: self.is_effect_shader(shape.shader_property),
            emissive: self.shader_emissive(shape.shader_property),
            bounding_sphere: Some(shape.bounding_sphere),
            skinned: false,
            parent_node: parent_name.to_string(),
            parent_transform: *parent,
            transform: parent.compose(&local),
        }
    }

    /// Reconstruct a skinned shape in bind pose from its
    /// `NiSkinPartition` (M12). Returns `None` when any skin link
    /// fails to resolve or the data is inconsistent; the caller
    /// falls back to the empty inline mesh.
    fn mesh_from_skin(
        &self,
        shape: &BsTriShapeBlock,
        dynamic_positions: Option<&[Vec3]>,
        parent: &Transform,
        parent_name: &str,
        node_worlds: &HashMap<i32, Transform>,
    ) -> Option<Mesh> {
        let Block::SkinInstance(instance) = self.block(shape.skin)? else {
            return None;
        };
        let Block::SkinPartition(partition) = self.block(instance.partition)? else {
            return None;
        };
        let Block::SkinData(skin_data) = self.block(instance.data)? else {
            return None;
        };
        let nverts = partition.vertices.len();
        if nverts == 0 {
            return None;
        }
        if let Some(dyn_pos) = dynamic_positions {
            // Partition vertex i pairs with dynamic vertex i.
            if dyn_pos.len() != nverts {
                return None;
            }
        }
        // Per-bone bind matrix B_b = BoneWorld_b ∘ BoneSkin_b ∘
        // GeomSkin, WITHOUT the shape transform (`Mesh.transform`
        // applies it later — linearity makes that identical to
        // baking the full matrix).
        let mut bone_mats: Vec<Transform> = Vec::with_capacity(instance.bones.len());
        for (bi, node_idx) in instance.bones.iter().enumerate() {
            let bone_world = node_worlds.get(node_idx).copied().unwrap_or_else(|| {
                // Bone node the tree walk never reached: use its
                // local transform, or identity when unresolvable.
                match self.block(*node_idx) {
                    Some(Block::Node(node)) => Transform {
                        translation: node.av.translation,
                        rotation: node.av.rotation,
                        scale: node.av.scale,
                    },
                    _ => Transform::identity(),
                }
            });
            let bone_skin = skin_data
                .bones
                .get(bi)
                .map(|b| b.transform)
                .unwrap_or_else(Transform::identity);
            bone_mats.push(
                bone_world
                    .compose(&bone_skin)
                    .compose(&skin_data.skin_transform),
            );
        }
        // Per-global-vertex weights: the first partition that lists
        // a vertex wins (partitions overlap at dismember seams).
        let mut weights: Vec<&[(u16, f32)]> = vec![&[]; nverts];
        for part in &partition.partitions {
            for (local_idx, w) in part.weights.iter().enumerate() {
                let global = part
                    .vertex_map
                    .get(local_idx)
                    .copied()
                    .unwrap_or(local_idx as u16) as usize;
                if global < nverts && weights[global].is_empty() {
                    weights[global] = w;
                }
            }
        }
        // Positions, transformed by the weighted bind matrices.
        let mut vertices: Vec<Vec3> = Vec::with_capacity(nverts);
        for (vi, v) in partition.vertices.iter().enumerate() {
            let pos = if let Some(dyn_pos) = dynamic_positions {
                dyn_pos[vi]
            } else {
                v.position.unwrap_or([0.0, 0.0, 0.0])
            };
            vertices.push(apply_weighted(&bone_mats, weights[vi], pos));
        }
        let attrs = bs_attributes(&partition.vertices);
        let raw_normals = attrs.normals;
        let uvs = attrs.uvs;
        let raw_tangents = attrs.tangents;
        let raw_bitangents = attrs.bitangents;
        let vertex_colors = attrs.vertex_colors;
        // Directions get the same weighted treatment, rotation-only,
        // renormalized. A zero-length result (opposing rotations
        // cancelling) keeps the raw direction rather than NaN.
        let rotate_all = |dirs: Vec<Vec3>| -> Vec<Vec3> {
            if dirs.is_empty() {
                return dirs;
            }
            dirs.iter()
                .enumerate()
                .map(|(vi, d)| rotate_weighted(&bone_mats, weights[vi], *d))
                .collect()
        };
        let normals = rotate_all(raw_normals);
        let tangents = rotate_all(raw_tangents);
        let bitangents = rotate_all(raw_bitangents);
        // Triangles: concatenated from all partitions; they index
        // GLOBAL vertices, so no remapping is needed.
        let mut indices: Vec<u32> = Vec::new();
        for part in &partition.partitions {
            for tri in &part.triangles {
                indices.extend(tri.iter().map(|v| u32::from(*v)));
            }
        }
        if vertices.is_empty() || indices.is_empty() {
            return None;
        }
        let local = Transform {
            translation: shape.av.translation,
            rotation: shape.av.rotation,
            scale: shape.av.scale,
        };
        Some(Mesh {
            name: shape.av.net.name.clone(),
            vertices,
            normals,
            uvs,
            tangents,
            bitangents,
            vertex_colors,
            indices,
            material_name: self.shader_name(shape.shader_property),
            textures: self.shader_textures(shape.shader_property),
            alpha_cutoff: self.alpha_cutoff(shape.alpha_property),
            alpha_blend: self.alpha_blend(shape.alpha_property),
            unlit: self.is_effect_shader(shape.shader_property),
            emissive: self.shader_emissive(shape.shader_property),
            bounding_sphere: Some(shape.bounding_sphere),
            skinned: true,
            parent_node: parent_name.to_string(),
            parent_transform: *parent,
            transform: parent.compose(&local),
        })
    }
}

/// The non-position attributes of a `BsVertex` array, extracted
/// together by [`bs_attributes`].
struct BsAttributes {
    normals: Vec<Vec3>,
    uvs: Vec<Vec2>,
    tangents: Vec<Vec3>,
    bitangents: Vec<Vec3>,
    vertex_colors: Vec<Color4>,
}

/// Extract the non-position attributes of a `BsVertex` array.
/// Attributes are uniform across a block, so the first vertex
/// decides whether an attribute array exists at all.
fn bs_attributes(vertices: &[BsVertex]) -> BsAttributes {
    let has = |f: fn(&BsVertex) -> Option<Vec3>| vertices.first().is_some_and(|v| f(v).is_some());
    let normals = if has(|v| v.normal) {
        vertices
            .iter()
            .map(|v| v.normal.unwrap_or([0.0, 0.0, 0.0]))
            .collect()
    } else {
        Vec::new()
    };
    let uvs = if vertices.first().is_some_and(|v| v.uv.is_some()) {
        vertices
            .iter()
            .map(|v| v.uv.unwrap_or([0.0, 0.0]))
            .collect()
    } else {
        Vec::new()
    };
    let tangents = if has(|v| v.tangent) {
        vertices
            .iter()
            .map(|v| v.tangent.unwrap_or([0.0, 0.0, 0.0]))
            .collect()
    } else {
        Vec::new()
    };
    let bitangents = if has(|v| v.tangent) {
        vertices
            .iter()
            .map(|v| {
                [
                    v.bitangent_x.unwrap_or(0.0),
                    v.bitangent_y.unwrap_or(0.0),
                    v.bitangent_z.unwrap_or(0.0),
                ]
            })
            .collect()
    } else {
        Vec::new()
    };
    let vertex_colors = if vertices.first().is_some_and(|v| v.color.is_some()) {
        vertices
            .iter()
            .map(|v| {
                let c = v.color.unwrap_or([255, 255, 255, 255]);
                [
                    f32::from(c[0]) / 255.0,
                    f32::from(c[1]) / 255.0,
                    f32::from(c[2]) / 255.0,
                    f32::from(c[3]) / 255.0,
                ]
            })
            .collect()
    } else {
        Vec::new()
    };
    BsAttributes {
        normals,
        uvs,
        tangents,
        bitangents,
        vertex_colors,
    }
}

/// Blend a point by bone weights: `Σ w_b × M_b × v`, normalized by
/// the weight sum. Empty weights or a zero sum return the point
/// unchanged (the shape transform still applies in the renderer).
fn apply_weighted(mats: &[Transform], weights: &[(u16, f32)], v: Vec3) -> Vec3 {
    let mut sum = 0.0f32;
    let mut out = [0.0f32; 3];
    for &(bone, w) in weights {
        let Some(mat) = mats.get(bone as usize) else {
            continue;
        };
        let p = mat.apply(v);
        out[0] += w * p[0];
        out[1] += w * p[1];
        out[2] += w * p[2];
        sum += w;
    }
    if sum > 0.0 {
        [out[0] / sum, out[1] / sum, out[2] / sum]
    } else {
        v
    }
}

/// Blend a direction by bone weights: `Σ w_b × R_b × v`,
/// normalized. Empty weights, a zero sum, or a zero-length result
/// return the direction unchanged.
fn rotate_weighted(mats: &[Transform], weights: &[(u16, f32)], v: Vec3) -> Vec3 {
    let mut sum = 0.0f32;
    let mut out = [0.0f32; 3];
    for &(bone, w) in weights {
        let Some(mat) = mats.get(bone as usize) else {
            continue;
        };
        let p = mat.rotate(v);
        out[0] += w * p[0];
        out[1] += w * p[1];
        out[2] += w * p[2];
        sum += w;
    }
    if sum <= 0.0 {
        return v;
    }
    let inv = 1.0 / sum;
    let len = ((out[0] * inv).powi(2) + (out[1] * inv).powi(2) + (out[2] * inv).powi(2)).sqrt();
    if len > 1e-6 {
        [out[0] * inv / len, out[1] * inv / len, out[2] * inv / len]
    } else {
        v
    }
}

/// The material name for a `NiGeometry` shape: the active material
/// from its Material Data, falling back to the first material, then
/// to the shader property's name.
fn material_name_for(geometry: &Geometry, shader_name: &str) -> String {
    let data = &geometry.material_data;
    if data.active_material >= 0 {
        if let Some(name) = data.names.get(data.active_material as usize) {
            if !name.is_empty() {
                return name.clone();
            }
        }
    }
    if let Some(name) = data.names.iter().find(|n| !n.is_empty()) {
        return name.clone();
    }
    shader_name.to_string()
}

/// A bounds-checked cursor over the NIF bytes.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn take(&mut self, len: usize, what: &'static str) -> Result<&'a [u8]> {
        if self.remaining() < len {
            return Err(NifError::Truncated(what));
        }
        let slice = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.take(1, what)?[0])
    }

    fn bool(&mut self, what: &'static str) -> Result<bool> {
        Ok(self.u8(what)? != 0)
    }

    fn u16(&mut self, what: &'static str) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2, what)?.try_into().unwrap()))
    }

    fn u32(&mut self, what: &'static str) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    fn u64(&mut self, what: &'static str) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8, what)?.try_into().unwrap()))
    }

    fn i32(&mut self, what: &'static str) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    fn f32(&mut self, what: &'static str) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    /// A 16-bit IEEE half float, widened to f32.
    fn hfloat(&mut self, what: &'static str) -> Result<f32> {
        Ok(f16_to_f32(self.u16(what)?))
    }

    /// A `normbyte`: a signed byte holding a float in [-1, 1].
    fn normbyte(&mut self, what: &'static str) -> Result<f32> {
        Ok(normbyte_to_f32(self.take(1, what)?[0] as i8))
    }

    fn vec2(&mut self, what: &'static str) -> Result<Vec2> {
        Ok([self.f32(what)?, self.f32(what)?])
    }

    fn vec3(&mut self, what: &'static str) -> Result<Vec3> {
        Ok([self.f32(what)?, self.f32(what)?, self.f32(what)?])
    }

    fn vec4(&mut self, what: &'static str) -> Result<Vec4> {
        Ok([
            self.f32(what)?,
            self.f32(what)?,
            self.f32(what)?,
            self.f32(what)?,
        ])
    }

    fn mat3(&mut self, what: &'static str) -> Result<Mat3> {
        Ok([
            [self.f32(what)?, self.f32(what)?, self.f32(what)?],
            [self.f32(what)?, self.f32(what)?, self.f32(what)?],
            [self.f32(what)?, self.f32(what)?, self.f32(what)?],
        ])
    }

    fn half_tex_coord(&mut self, what: &'static str) -> Result<Vec2> {
        Ok([self.hfloat(what)?, self.hfloat(what)?])
    }

    fn half_vec3(&mut self, what: &'static str) -> Result<Vec3> {
        Ok([self.hfloat(what)?, self.hfloat(what)?, self.hfloat(what)?])
    }

    fn byte_vec3(&mut self, what: &'static str) -> Result<Vec3> {
        Ok([
            self.normbyte(what)?,
            self.normbyte(what)?,
            self.normbyte(what)?,
        ])
    }

    fn triangle(&mut self, what: &'static str) -> Result<[u16; 3]> {
        Ok([self.u16(what)?, self.u16(what)?, self.u16(what)?])
    }

    /// A line ending in '\n' (the newline is consumed, not returned).
    fn line(&mut self, what: &'static str) -> Result<String> {
        let rest = &self.bytes[self.pos..];
        let Some(end) = rest.iter().position(|b| *b == b'\n') else {
            return Err(NifError::Truncated(what));
        };
        let text = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.pos += end + 1;
        Ok(text)
    }

    /// A `SizedString`: u32 length + bytes, no NUL terminator.
    fn sized_string(&mut self, what: &'static str) -> Result<String> {
        let len = self.u32(what)? as usize;
        let bytes = self.take(len, what)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    /// An `ExportString`: byte length (including the NUL terminator)
    /// + bytes.
    fn export_string(&mut self, what: &'static str) -> Result<String> {
        let len = self.u8(what)? as usize;
        let bytes = self.take(len, what)?;
        let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    /// A `string` / `FilePath` field: a u32 index into the header
    /// string table, resolved to the string itself.
    fn string(&mut self, strings: &[String], what: &'static str) -> Result<String> {
        let index = self.u32(what)?;
        resolve_string(strings, index)
    }
}

/// Resolve a string-table index. `0xFFFFFFFF` is the empty string;
/// anything else out of range is an error.
fn resolve_string(strings: &[String], index: u32) -> Result<String> {
    if index == 0xFFFF_FFFF {
        return Ok(String::new());
    }
    strings
        .get(index as usize)
        .cloned()
        .ok_or(NifError::InvalidData("string index out of range"))
}

// ---------------------------------------------------------------------------
// Havok collision block parsers. Every parser reads from the
// size-bounded block reader and fails on truncation or invalid
// values; `parse_block` degrades any failure to `Block::Unknown`.
// Offsets are from the nif.xml layouts, verified against real
// files (see the struct docs).
// ---------------------------------------------------------------------------

/// Caps, so a corrupt count cannot allocate the box: shapes per
/// file, vertices per shape, children per list.
const MAX_COLLISION_SHAPES: usize = 256;
const MAX_COLLISION_VERTICES: usize = 1_000_000;
/// `bhkMultiSphereShape` sphere count (nif.xml range 1:8).
const MAX_SPHERES: usize = 8;

fn parse_collision_object(reader: &mut Reader) -> Result<CollisionObjectBlock> {
    let target = reader.i32("collision target")?;
    let _flags = reader.u16("collision flags")?;
    let body = reader.i32("collision body")?;
    Ok(CollisionObjectBlock { target, body })
}

fn parse_rigid_body(reader: &mut Reader, has_transform: bool) -> Result<RigidBodyBlock> {
    let shape = reader.i32("rigid body shape")?;
    let layer = reader.u8("rigid body layer")?;
    let flags = reader.u8("rigid body filter flags")?;
    let _group = reader.u16("rigid body group")?;
    // World-object info @8..28 (20 bytes), then the first
    // collision response @28.
    reader.take(20, "world object info")?;
    let response = reader.u8("collision response")?;
    // Entity info tail @29..32 (3 bytes), then CInfo2010: unused
    // @32..36 (4 bytes), second filter @36..40.
    reader.take(3, "entity info tail")?;
    reader.take(4, "cinfo unused")?;
    let _layer2 = reader.u8("rigid body layer copy")?;
    let flags2 = reader.u8("rigid body filter flags copy")?;
    let _group2 = reader.u16("rigid body group copy")?;
    // Unused @40..44 (4 bytes), unknown int @44..48 (4 bytes),
    // second response @48.
    reader.take(8, "cinfo unused 2")?;
    let response2 = reader.u8("collision response copy")?;
    let collides = flags & 0x40 == 0
        && flags2 & 0x40 == 0
        && !matches!(response, 2 | 3)
        && !matches!(response2, 2 | 3);
    let transform = if has_transform {
        // Translation @52 (3 bytes in: response2 was @48, then
        // 3 bytes of the CInfo tail @49..52). Havok stores the
        // translation as a Vector4, so 4 more bytes (w) follow
        // before the rotation quaternion @68.
        reader.take(3, "cinfo tail")?;
        let translation = reader.vec3("rigid body translation")?;
        let _w = reader.f32("rigid body translation w")?;
        let quat = reader.vec4("rigid body rotation")?;
        let len_sq = quat[0] * quat[0] + quat[1] * quat[1] + quat[2] * quat[2] + quat[3] * quat[3];
        if !len_sq.is_finite() || len_sq < 0.5 || len_sq > 1.5 {
            return Err(NifError::InvalidData("rigid body quaternion"));
        }
        if !translation.iter().all(|v| v.is_finite()) {
            return Err(NifError::InvalidData("rigid body translation"));
        }
        Some(Transform {
            translation: [
                translation[0] * HAVOK_TO_SKYRIM,
                translation[1] * HAVOK_TO_SKYRIM,
                translation[2] * HAVOK_TO_SKYRIM,
            ],
            rotation: quat_to_mat3(quat),
            scale: 1.0,
        })
    } else {
        None
    };
    Ok(RigidBodyBlock {
        shape,
        layer,
        collides,
        transform,
    })
}

fn parse_mopp_shape(reader: &mut Reader) -> Result<MoppShapeBlock> {
    let child = reader.i32("mopp child")?;
    Ok(MoppShapeBlock { child })
}

fn parse_transform_shape(reader: &mut Reader, convex: bool) -> Result<TransformShapeBlock> {
    let child = reader.i32("transform shape child")?;
    // Transform type + unused: 16 bytes @4..20 (a u32 material
    // + 12 bytes of filter/info), then the 4x4 matrix @20.
    reader.take(16, "transform shape header")?;
    let mut matrix = [0.0f32; 16];
    for value in matrix.iter_mut() {
        *value = reader.f32("transform shape matrix")?;
    }
    if !matrix.iter().all(|v| v.is_finite()) {
        return Err(NifError::InvalidData("transform shape matrix"));
    }
    // Column-major translation (columns[12..15]) is in metres.
    matrix[12] *= HAVOK_TO_SKYRIM;
    matrix[13] *= HAVOK_TO_SKYRIM;
    matrix[14] *= HAVOK_TO_SKYRIM;
    Ok(TransformShapeBlock {
        child,
        matrix,
        convex,
    })
}

fn parse_list_shape(reader: &mut Reader) -> Result<ListShapeBlock> {
    let count = reader.u32("list shape count")? as usize;
    if count > MAX_COLLISION_SHAPES {
        return Err(NifError::InvalidData("list shape count"));
    }
    let mut children = Vec::with_capacity(count);
    for _ in 0..count {
        children.push(reader.i32("list shape child")?);
    }
    Ok(ListShapeBlock { children })
}

fn parse_compressed_mesh_shape(reader: &mut Reader) -> Result<CompressedMeshShapeBlock> {
    // Material + filter/info @0..16, scale @16, then radius,
    // unknown and filter fields @28..52, data ref @52.
    reader.take(16, "compressed mesh header")?;
    let scale = reader.vec3("compressed mesh scale")?;
    if !scale.iter().all(|v| v.is_finite() && *v > 0.0) {
        return Err(NifError::InvalidData("compressed mesh scale"));
    }
    reader.take(24, "compressed mesh tail")?;
    let data = reader.i32("compressed mesh data")?;
    Ok(CompressedMeshShapeBlock { scale, data })
}

fn parse_ni_tri_strips_shape(reader: &mut Reader) -> Result<NiTriStripsShapeBlock> {
    // Material + filter/info + transform @0..32, scale @32,
    // then unused @44..48, count @48, data refs @52+4i.
    reader.take(32, "tri strips shape header")?;
    let scale = reader.vec3("tri strips shape scale")?;
    if !scale.iter().all(|v| v.is_finite() && *v > 0.0) {
        return Err(NifError::InvalidData("tri strips shape scale"));
    }
    reader.take(4, "tri strips shape unused")?;
    let count = reader.u32("tri strips shape count")? as usize;
    if count > MAX_COLLISION_SHAPES {
        return Err(NifError::InvalidData("tri strips shape count"));
    }
    let mut data_blocks = Vec::with_capacity(count);
    for _ in 0..count {
        data_blocks.push(reader.i32("tri strips shape data")?);
    }
    Ok(NiTriStripsShapeBlock { scale, data_blocks })
}

fn parse_capsule_shape(reader: &mut Reader) -> Result<CapsuleShapeBlock> {
    // Material + filter/info @0..16, A @16, radius @28, B @32,
    // radius @44.
    reader.take(16, "capsule header")?;
    let a = reader.vec3("capsule A")?;
    let r1 = reader.f32("capsule radius A")?;
    let b = reader.vec3("capsule B")?;
    let r2 = reader.f32("capsule radius B")?;
    let radius = r1.max(r2);
    if !radius.is_finite()
        || radius <= 0.0
        || !a.iter().all(|v| v.is_finite())
        || !b.iter().all(|v| v.is_finite())
    {
        return Err(NifError::InvalidData("capsule shape"));
    }
    Ok(CapsuleShapeBlock { a, b, radius })
}

fn parse_sphere_shape(reader: &mut Reader) -> Result<SphereShapeBlock> {
    let _material = reader.u32("sphere material")?;
    let radius = reader.f32("sphere radius")?;
    if !radius.is_finite() || radius <= 0.0 {
        return Err(NifError::InvalidData("sphere radius"));
    }
    Ok(SphereShapeBlock { radius })
}

fn parse_cylinder_shape(reader: &mut Reader) -> Result<CylinderShapeBlock> {
    // Material @0, shell radius @4, unused @8, A Vector4 @16,
    // B Vector4 @32, radius @48.
    reader.take(16, "cylinder header")?;
    let a4 = reader.vec4("cylinder A")?;
    let b4 = reader.vec4("cylinder B")?;
    let radius = reader.f32("cylinder radius")?;
    if !radius.is_finite()
        || radius <= 0.0
        || !a4[..3].iter().all(|v| v.is_finite())
        || !b4[..3].iter().all(|v| v.is_finite())
    {
        return Err(NifError::InvalidData("cylinder shape"));
    }
    Ok(CylinderShapeBlock {
        a: [a4[0], a4[1], a4[2]],
        b: [b4[0], b4[1], b4[2]],
        radius,
    })
}

fn parse_multi_sphere_shape(reader: &mut Reader) -> Result<MultiSphereShapeBlock> {
    // Material @0, world-object info @4..16, count @16, then
    // NiBound { centre vec3, radius f32 } x n from @20.
    reader.take(16, "multi sphere header")?;
    let count = reader.u32("multi sphere count")? as usize;
    if !(1..=MAX_SPHERES).contains(&count) {
        return Err(NifError::InvalidData("multi sphere count"));
    }
    let mut spheres = Vec::with_capacity(count);
    for _ in 0..count {
        let center = reader.vec3("multi sphere centre")?;
        let radius = reader.f32("multi sphere radius")?;
        if !radius.is_finite() || radius <= 0.0 || !center.iter().all(|v| v.is_finite()) {
            return Err(NifError::InvalidData("multi sphere"));
        }
        spheres.push((center, radius));
    }
    Ok(MultiSphereShapeBlock { spheres })
}

fn parse_box_shape(reader: &mut Reader) -> Result<BoxShapeBlock> {
    // Material + filter/info @0..16, half-extents @16.
    reader.take(16, "box header")?;
    let half_extents = reader.vec3("box half extents")?;
    if !half_extents.iter().all(|v| v.is_finite() && *v > 0.0) {
        return Err(NifError::InvalidData("box half extents"));
    }
    Ok(BoxShapeBlock { half_extents })
}

fn parse_convex_vertices_shape(reader: &mut Reader) -> Result<ConvexVerticesShapeBlock> {
    // Material + filter/info + radius/centre @0..32, count @32,
    // vertices (Vector4, 16-byte stride) from @36.
    reader.take(32, "convex vertices header")?;
    let count = reader.u32("convex vertices count")? as usize;
    if !(4..=MAX_COLLISION_VERTICES).contains(&count) {
        return Err(NifError::InvalidData("convex vertices count"));
    }
    let mut points = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        let v = reader.vec4("convex vertex")?;
        if !v[..3].iter().all(|f| f.is_finite()) {
            return Err(NifError::InvalidData("convex vertex"));
        }
        points.push([v[0], v[1], v[2]]);
    }
    Ok(ConvexVerticesShapeBlock { points })
}

/// A decoded `bhkCompressedMeshShapeData`: vertices in the data
/// block's local space (chunk transforms applied, metres already
/// converted to Skyrim units) and triangles grouped per material.
/// The shape-level frame (node composition, body transform,
/// shape scale) is applied by the caller.
struct DecodedCompressedMesh {
    vertices: Vec<Vec3>,
    /// (Havok material code, material layer, triangles) per
    /// material that owns at least one physical triangle.
    groups: Vec<(u32, u8, Vec<[u32; 3]>)>,
}

/// Decode a `bhkCompressedMeshShapeData` block. Returns `None`
/// on any layout violation (unsupported index format, named
/// materials, referenced chunks, convex pieces, trailing bytes,
/// out-of-range indices, empty result) — the caller skips that
/// shape.
///
/// Layout (verified byte by byte against WalkwayStairs1.nif):
/// bits u32 (17), winding bits u32 (18), index/winding masks
/// (8 bytes), quantization error f32, AABB (32 bytes),
/// welding/material mode (2 bytes), three count+array pairs
/// (widths 4/2/1 — the arrays are skipped), materials (count,
/// then per material: Havok material code u32, layer u8, 3 pad
/// bytes), named materials (count, must be 0), chunk
/// transforms (count, then per transform: translation vec3 +
/// w, rotation quat), big vertices (count, then vec3 + w each),
/// big triangles (count, then 3x u16 indices + material u32 +
/// 2 pad bytes each), chunks (count, then per chunk:
/// translation vec3 + w, material u32, reference u16 (must be
/// 0xFFFF — referenced chunks are unsupported), transform
/// index u16, quantized vertices (count divisible by 3, 3x u16
/// each, scaled by 70 x quantization error), indices (u16),
/// strip lengths (u16), weld indices (u16, skipped); strips
/// triangulate with alternating winding and the leftover
/// indices form plain triangles), convex pieces (count, must
/// be 0). Triangles whose material layer is NONCOLLIDABLE (15)
/// or that are degenerate are dropped.
fn decode_compressed_mesh(bytes: &[u8]) -> Option<DecodedCompressedMesh> {
    decode_compressed_mesh_inner(bytes).ok()
}

fn decode_compressed_mesh_inner(bytes: &[u8]) -> Result<DecodedCompressedMesh> {
    let mut reader = Reader::new(bytes);
    let invalid = |what: &'static str| NifError::InvalidData(what);
    let bits = reader.u32("compressed bits")?;
    let winding_bits = reader.u32("compressed winding bits")?;
    if bits != 17 || winding_bits != 18 {
        return Err(invalid("compressed mesh index format"));
    }
    reader.take(8, "compressed masks")?;
    let quant_error = reader.f32("compressed quantization error")?;
    if !quant_error.is_finite() || quant_error <= 0.0 {
        return Err(invalid("compressed quantization error"));
    }
    reader.take(32 + 2, "compressed bounds")?;
    for width in [4usize, 2, 1] {
        let count = reader.u32("compressed array count")? as usize;
        if count > MAX_COLLISION_VERTICES {
            return Err(invalid("compressed array count"));
        }
        reader.take(count * width, "compressed array")?;
    }
    let material_count = reader.u32("compressed materials")? as usize;
    if material_count > MAX_COLLISION_SHAPES {
        return Err(invalid("compressed material count"));
    }
    let mut material_codes: Vec<u32> = Vec::with_capacity(material_count);
    let mut material_layers: Vec<u8> = Vec::with_capacity(material_count);
    for _ in 0..material_count {
        material_codes.push(reader.u32("compressed material code")?);
        material_layers.push(reader.u8("compressed material layer")?);
        reader.take(3, "compressed material pad")?;
    }
    let named = reader.u32("compressed named materials")?;
    if named != 0 {
        return Err(invalid("named compressed materials"));
    }
    let transform_count = reader.u32("compressed transforms")? as usize;
    if transform_count > MAX_COLLISION_SHAPES {
        return Err(invalid("compressed transform count"));
    }
    let mut chunk_transforms: Vec<Mat4> = Vec::with_capacity(transform_count);
    for _ in 0..transform_count {
        let translation = reader.vec3("chunk transform translation")?;
        let _w = reader.f32("chunk transform w")?;
        let quat = reader.vec4("chunk transform rotation")?;
        let len_sq = quat[0] * quat[0] + quat[1] * quat[1] + quat[2] * quat[2] + quat[3] * quat[3];
        if !len_sq.is_finite() || len_sq < 0.5 || len_sq > 1.5 {
            return Err(invalid("chunk transform rotation"));
        }
        if !translation.iter().all(|v| v.is_finite()) {
            return Err(invalid("chunk transform translation"));
        }
        let m = mat4_from_transform(&Transform {
            translation: [
                translation[0] * HAVOK_TO_SKYRIM,
                translation[1] * HAVOK_TO_SKYRIM,
                translation[2] * HAVOK_TO_SKYRIM,
            ],
            rotation: quat_to_mat3(quat),
            scale: 1.0,
        });
        chunk_transforms.push(m);
    }
    let big_vertices = reader.u32("compressed big vertices")? as usize;
    if big_vertices > MAX_COLLISION_VERTICES {
        return Err(invalid("compressed big vertex count"));
    }
    let mut vertices: Vec<Vec3> = Vec::with_capacity(big_vertices.min(4096));
    for _ in 0..big_vertices {
        let p = reader.vec3("compressed big vertex")?;
        let _w = reader.f32("compressed big vertex w")?;
        if !p.iter().all(|v| v.is_finite()) {
            return Err(invalid("compressed big vertex"));
        }
        vertices.push([
            p[0] * HAVOK_TO_SKYRIM,
            p[1] * HAVOK_TO_SKYRIM,
            p[2] * HAVOK_TO_SKYRIM,
        ]);
    }
    let mut per_material: Vec<Vec<[u32; 3]>> = vec![Vec::new(); material_count];
    let big_triangles = reader.u32("compressed big triangles")? as usize;
    if big_triangles > MAX_COLLISION_VERTICES {
        return Err(invalid("compressed big triangle count"));
    }
    for _ in 0..big_triangles {
        let tri = [
            u32::from(reader.u16("compressed triangle index")?),
            u32::from(reader.u16("compressed triangle index")?),
            u32::from(reader.u16("compressed triangle index")?),
        ];
        let material = reader.u32("compressed triangle material")? as usize;
        reader.take(2, "compressed triangle pad")?;
        if material >= material_count {
            return Err(invalid("compressed triangle material"));
        }
        if tri.iter().any(|&i| i as usize >= big_vertices) {
            return Err(invalid("compressed triangle index"));
        }
        if material_layers[material] != LAYER_NONCOLLIDABLE
            && tri[0] != tri[1]
            && tri[1] != tri[2]
            && tri[0] != tri[2]
        {
            per_material[material].push(tri);
        }
    }
    let chunk_count = reader.u32("compressed chunks")? as usize;
    if chunk_count > MAX_COLLISION_SHAPES {
        return Err(invalid("compressed chunk count"));
    }
    for _ in 0..chunk_count {
        let chunk_translation = reader.vec3("chunk translation")?;
        let _w = reader.f32("chunk translation w")?;
        let material = reader.u32("chunk material")? as usize;
        let reference = reader.u16("chunk reference")?;
        let transform_index = reader.u16("chunk transform index")? as usize;
        if reference != u16::MAX {
            return Err(invalid("referenced compressed chunk"));
        }
        if material >= material_count {
            return Err(invalid("chunk material"));
        }
        let Some(chunk_transform) = chunk_transforms.get(transform_index) else {
            return Err(invalid("chunk transform index"));
        };
        let chunk_transform = *chunk_transform;
        if !chunk_translation.iter().all(|v| v.is_finite()) {
            return Err(invalid("chunk translation"));
        }
        let chunk_translation = [
            chunk_translation[0] * HAVOK_TO_SKYRIM,
            chunk_translation[1] * HAVOK_TO_SKYRIM,
            chunk_translation[2] * HAVOK_TO_SKYRIM,
        ];
        let coordinate_count = reader.u32("chunk coordinates")? as usize;
        if !coordinate_count.is_multiple_of(3) || coordinate_count / 3 > MAX_COLLISION_VERTICES {
            return Err(invalid("chunk coordinate count"));
        }
        let count = coordinate_count / 3;
        if vertices.len() + count > MAX_COLLISION_VERTICES {
            return Err(invalid("compressed vertex limit"));
        }
        let offset = vertices.len() as u32;
        let quantum = HAVOK_TO_SKYRIM * quant_error;
        for _ in 0..count {
            let q = [
                f32::from(reader.u16("chunk vertex")?) * quantum,
                f32::from(reader.u16("chunk vertex")?) * quantum,
                f32::from(reader.u16("chunk vertex")?) * quantum,
            ];
            let p = [
                chunk_translation[0] + q[0],
                chunk_translation[1] + q[1],
                chunk_translation[2] + q[2],
            ];
            vertices.push(mat4_point(&chunk_transform, p));
        }
        let index_count = reader.u32("chunk indices")? as usize;
        if index_count > MAX_COLLISION_VERTICES * 3 {
            return Err(invalid("chunk index count"));
        }
        let mut indices: Vec<u32> = Vec::with_capacity(index_count.min(4096));
        for _ in 0..index_count {
            let index = reader.u16("chunk index")?;
            if index as usize >= count {
                return Err(invalid("chunk index"));
            }
            indices.push(u32::from(index) + offset);
        }
        let strip_count = reader.u32("chunk strips")? as usize;
        if strip_count > MAX_COLLISION_VERTICES {
            return Err(invalid("chunk strip count"));
        }
        let mut lengths: Vec<usize> = Vec::with_capacity(strip_count.min(4096));
        for _ in 0..strip_count {
            lengths.push(usize::from(reader.u16("chunk strip length")?));
        }
        let weld = reader.u32("chunk weld")? as usize;
        if weld > MAX_COLLISION_VERTICES * 3 {
            return Err(invalid("chunk weld count"));
        }
        reader.take(weld * 2, "chunk weld indices")?;
        let physical = material_layers[material] != LAYER_NONCOLLIDABLE;
        let mut used = 0usize;
        for length in lengths {
            if length < 3 || used + length > indices.len() {
                return Err(invalid("chunk strip length"));
            }
            if physical {
                for i in 0..length - 2 {
                    let tri = if i % 2 == 0 {
                        [
                            indices[used + i],
                            indices[used + i + 1],
                            indices[used + i + 2],
                        ]
                    } else {
                        [
                            indices[used + i + 1],
                            indices[used + i],
                            indices[used + i + 2],
                        ]
                    };
                    if tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2] {
                        per_material[material].push(tri);
                    }
                }
            }
            used += length;
        }
        if !(indices.len() - used).is_multiple_of(3) {
            return Err(invalid("chunk triangle tail"));
        }
        if physical {
            for tail in indices[used..].as_chunks::<3>().0 {
                let tri = [tail[0], tail[1], tail[2]];
                if tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2] {
                    per_material[material].push(tri);
                }
            }
        }
    }
    let convex_pieces = reader.u32("compressed convex pieces")?;
    if convex_pieces != 0 || reader.remaining() != 0 {
        return Err(invalid("compressed mesh tail"));
    }
    let mut groups = Vec::new();
    for (i, triangles) in per_material.into_iter().enumerate() {
        if triangles.is_empty() {
            continue;
        }
        groups.push((material_codes[i], material_layers[i], triangles));
    }
    if groups.is_empty() {
        return Err(invalid("compressed mesh has no physical triangles"));
    }
    Ok(DecodedCompressedMesh { vertices, groups })
}

/// Parse one block from its (size-bounded) bytes. Unknown types are
/// returned as [`Block::Unknown`] without reading their bytes.
/// Skinning blocks parse only for BS version 100 (Skyrim SE) — the
/// SE layouts are proven byte-exact on real files, the LE layouts
/// are unverified and no LE file in Skyrim SE's BSAs carries
/// skinning, so LE skin blocks keep the old Unknown behavior.
fn parse_block(
    type_name: &str,
    bytes: &[u8],
    strings: &[String],
    bs_version: u32,
) -> Result<Block> {
    let mut reader = Reader::new(bytes);
    Ok(match type_name {
        "NiNode" => Block::Node(parse_node(&mut reader, strings)?),
        // BSFadeNode is a NiNode subclass with no extra serialized
        // fields — same layout, so the node parser reads it. It is
        // the standard root of most Skyrim SE meshes; treating it
        // as Unknown orphaned every shape under it (M11 door bug:
        // the panel's parent node was invisible to the tree walk).
        "BSFadeNode" => Block::Node(parse_node(&mut reader, strings)?),
        // BSTreeNode (M13) is the standard root of tree NIFs: a
        // NiNode prefix plus a 44-byte tail (11 u32s) that the
        // size-bounded reader skips. Treating it as Unknown died
        // the tree walk at block 0 for every pine.
        "BSTreeNode" => Block::Node(parse_node(&mut reader, strings)?),
        // BSMultiBoundNode (M13): a NiNode prefix plus one i32
        // ref to its BSMultiBound (bounds data we do not use).
        // It sits BETWEEN the BSTreeNode root and the switch in
        // every pine NIF (root children [2], block 2's children
        // include the NiSwitchNode) — without it the walk still
        // dies before reaching the switch, and both trees render
        // via the fallback. The research doc's §3 missed this
        // block; the real pine file proves it.
        "BSMultiBoundNode" => {
            let node = parse_node(&mut reader, strings)?;
            let _multibound = reader.i32("multi bound ref")?;
            Block::Node(node)
        }
        // BSOrderedNode (M13): a NiNode with NO extra fields —
        // a pure type marker telling the renderer to draw its
        // children in order (transparency sorting). Effect NIFs
        // hang their meshes under it (the waterfall's fallsMesh
        // shapes are children of block 7 in
        // FXWaterfallThin2048x128.nif); as Unknown the walk died
        // there and the shapes rendered via the fallback. The
        // fallback happened to place them right (the ordered
        // node's transform is identity), but the walk should
        // reach them properly.
        "BSOrderedNode" => Block::Node(parse_node(&mut reader, strings)?),
        // NiSwitchNode (M13): NiNode prefix + u16 flags + i32
        // active-child index (6 trailing bytes). Only the active
        // child renders (see `meshes()`).
        "NiSwitchNode" => {
            let node = parse_node(&mut reader, strings)?;
            let _flags = reader.u16("switch flags")?;
            let active_child = reader.i32("switch active child")?;
            Block::SwitchNode(SwitchNodeBlock { node, active_child })
        }
        "NiTriShape" => Block::TriShape(TriShapeBlock {
            geometry: parse_geometry(&mut reader, strings)?,
        }),
        "NiTriStrips" => Block::TriStrips(TriStripsBlock {
            geometry: parse_geometry(&mut reader, strings)?,
        }),
        "BSLODTriShape" => Block::BsLodTriShape(BsLodTriShapeBlock {
            geometry: parse_geometry(&mut reader, strings)?,
            lod_sizes: [
                reader.u32("LOD0 size")?,
                reader.u32("LOD1 size")?,
                reader.u32("LOD2 size")?,
            ],
        }),
        "BSTriShape" => Block::BsTriShape(Box::new(parse_bs_tri_shape(&mut reader, strings)?)),
        "BSDynamicTriShape" => {
            let shape = parse_bs_tri_shape(&mut reader, strings)?;
            let (dynamic_positions, dynamic_data_size) = parse_dynamic_data(&mut reader, &shape)?;
            let trailing_bytes = reader.remaining();
            Block::BsDynamicTriShape(Box::new(BsDynamicTriShapeBlock {
                shape,
                dynamic_positions,
                dynamic_data_size,
                trailing_bytes,
            }))
        }
        "NiTriShapeData" => {
            Block::TriShapeData(Box::new(parse_tri_shape_data(&mut reader, strings)?))
        }
        "NiTriStripsData" => {
            Block::TriStripsData(Box::new(parse_tri_strips_data(&mut reader, strings)?))
        }
        "NiTexturingProperty" => {
            Block::TexturingProperty(Box::new(parse_texturing_property(&mut reader, strings)?))
        }
        "NiSourceTexture" => Block::SourceTexture(parse_source_texture(&mut reader, strings)?),
        "BSLightingShaderProperty" => {
            Block::LightingShaderProperty(parse_lighting_shader_property(&mut reader, strings)?)
        }
        // BSEffectShaderProperty (exterior follow-up): fog, water
        // sheets, glows. A parse failure degrades to Unknown like
        // the animation blocks — an effect shader this parser does
        // not know must not take the meshes down with it; the
        // shape then renders lit and opaque (the old behavior).
        "BSEffectShaderProperty" => parse_effect_shader_property(&mut reader, strings)
            .map(Block::EffectShaderProperty)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "BSShaderTextureSet" => Block::ShaderTextureSet(parse_shader_texture_set(&mut reader)?),
        "NiAlphaProperty" => Block::AlphaProperty(parse_alpha_property(&mut reader, strings)?),
        "NiMaterialProperty" => {
            Block::MaterialProperty(parse_material_property(&mut reader, strings)?)
        }
        // Skinning (M12): SE layouts only, proven byte-exact on
        // real files (see the parser docs).
        "NiSkinInstance" if bs_version == BS_VERSION_SKYRIM_SE => {
            Block::SkinInstance(parse_skin_instance(&mut reader, false)?)
        }
        "BSDismemberSkinInstance" if bs_version == BS_VERSION_SKYRIM_SE => {
            Block::SkinInstance(parse_skin_instance(&mut reader, true)?)
        }
        "NiSkinData" if bs_version == BS_VERSION_SKYRIM_SE => {
            Block::SkinData(Box::new(parse_skin_data(&mut reader)?))
        }
        "NiSkinPartition" if bs_version == BS_VERSION_SKYRIM_SE => {
            Block::SkinPartition(Box::new(parse_skin_partition(&mut reader)?))
        }
        // NiTransformData (M15): XYZ rotation keys only. A parse
        // failure degrades to Unknown instead of failing the whole
        // file — animation data is never load-bearing for rendering,
        // and a layout this parser does not know must not take the
        // meshes down with it.
        "NiTransformData" => parse_transform_data(&mut reader)
            .map(Block::TransformData)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        // NiControllerSequence (secret door): the Open/Close
        // sequences and their controlled blocks — which
        // interpolator drives which node. A parse failure
        // degrades to Unknown like TransformData: animation data
        // is never load-bearing for rendering.
        "NiControllerSequence" => parse_controller_sequence(&mut reader, strings)
            .map(Block::ControllerSequence)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        // NiTransformInterpolator (secret door): the bridge from
        // a controlled block to its NiTransformData. Same
        // degrade-to-Unknown rule.
        "NiTransformInterpolator" => parse_transform_interpolator(&mut reader)
            .map(Block::TransformInterpolator)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        // NiTransformController (sawmill): a keyframed animation
        // attached directly to a node. Same degrade-to-Unknown
        // rule.
        "NiTransformController" => parse_transform_controller(&mut reader)
            .map(Block::TransformController)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        // Havok collision (per-object collision milestone): every
        // arm degrades to Unknown on a parse failure — collision
        // is never load-bearing for rendering, and a layout this
        // parser does not know must not take the meshes down with
        // it. Constraint blocks (bhkHingeConstraint,
        // bhkLimitedHingeConstraint, bhkBreakableConstraint) and
        // phantom volumes (bhkSPCollisionObject,
        // bhkSimpleShapePhantom) carry no collision geometry and
        // stay Unknown by design.
        "bhkCollisionObject" => parse_collision_object(&mut reader)
            .map(Block::CollisionObject)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "bhkRigidBody" => parse_rigid_body(&mut reader, false)
            .map(Block::RigidBody)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "bhkRigidBodyT" => parse_rigid_body(&mut reader, true)
            .map(Block::RigidBody)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "bhkMoppBvTreeShape" => parse_mopp_shape(&mut reader)
            .map(Block::MoppShape)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "bhkTransformShape" => parse_transform_shape(&mut reader, false)
            .map(Block::TransformShape)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "bhkConvexTransformShape" => parse_transform_shape(&mut reader, true)
            .map(Block::TransformShape)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "bhkListShape" => parse_list_shape(&mut reader)
            .map(Block::ListShape)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "bhkCompressedMeshShape" => parse_compressed_mesh_shape(&mut reader)
            .map(Block::CompressedMeshShape)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "bhkCompressedMeshShapeData" => Block::CompressedMeshShapeData(CompressedMeshDataBlock {
            bytes: bytes.to_vec(),
        }),
        "bhkNiTriStripsShape" => parse_ni_tri_strips_shape(&mut reader)
            .map(Block::NiTriStripsShape)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "bhkCapsuleShape" => parse_capsule_shape(&mut reader)
            .map(Block::CapsuleShape)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "bhkSphereShape" => parse_sphere_shape(&mut reader)
            .map(Block::SphereShape)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "bhkCylinderShape" => parse_cylinder_shape(&mut reader)
            .map(Block::CylinderShape)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "bhkMultiSphereShape" => parse_multi_sphere_shape(&mut reader)
            .map(Block::MultiSphereShape)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        "bhkBoxShape" => {
            parse_box_shape(&mut reader)
                .map(Block::BoxShape)
                .unwrap_or(Block::Unknown {
                    type_name: type_name.to_string(),
                })
        }
        "bhkConvexVerticesShape" => parse_convex_vertices_shape(&mut reader)
            .map(Block::ConvexVerticesShape)
            .unwrap_or(Block::Unknown {
                type_name: type_name.to_string(),
            }),
        _ => Block::Unknown {
            type_name: type_name.to_string(),
        },
    })
}

/// Key-group interpolation types (nif.xml KeyType).
const KEY_CONSTANT: u32 = 0;
const KEY_LINEAR: u32 = 1;
const KEY_QUADRATIC: u32 = 2;
/// Rotation type 4 = XYZ Euler (three per-axis float groups).
const ROTATION_XYZ: u32 = 4;

/// Parse one float key group: u32 numKeys, u32 key type, then the
/// keys. Constant keys carry only the value (time 0.0); linear
/// keys (time, value); quadratic keys (time, value, BACKWARD
/// tangent, FORWARD tangent) — the file order is backward-first
/// (proven by the grindstone/blade tangent continuity, sawmill
/// milestone). Returns the keys plus the group's interpolation
/// type, which the evaluator needs.
fn parse_float_key_group(reader: &mut Reader) -> Result<(Vec<FloatKey>, KeyInterpolation)> {
    let num = reader.u32("num keys")? as usize;
    let key_type = reader.u32("key type")?;
    let interp = KeyInterpolation::from_u32(key_type)
        .ok_or(NifError::InvalidData("unknown key interpolation type"))?;
    let mut keys = Vec::with_capacity(num.min(1024));
    for _ in 0..num {
        let key = match key_type {
            KEY_CONSTANT => FloatKey {
                time: 0.0,
                value: reader.f32("constant key value")?,
                fwd_tangent: 0.0,
                bwd_tangent: 0.0,
            },
            KEY_LINEAR => FloatKey {
                time: reader.f32("linear key time")?,
                value: reader.f32("linear key value")?,
                fwd_tangent: 0.0,
                bwd_tangent: 0.0,
            },
            KEY_QUADRATIC => FloatKey {
                time: reader.f32("quadratic key time")?,
                value: reader.f32("quadratic key value")?,
                // File order is backward tangent FIRST.
                bwd_tangent: reader.f32("quadratic key bwd tangent")?,
                fwd_tangent: reader.f32("quadratic key fwd tangent")?,
            },
            _ => return Err(NifError::InvalidData("unknown key interpolation type")),
        };
        keys.push(key);
    }
    Ok((keys, interp))
}

/// Parse one translation (vec3 values) key group: u32 numKeys,
/// then — only when the count is nonzero — u32 key type and the
/// keys. Mirrors [`parse_float_key_group`] with 3 floats per
/// value (constant: value only; linear: time + value;
/// quadratic: time + value + bwd/fwd tangent vectors, file order
/// backward-first). An empty group returns `Constant` as the
/// type (no type field exists to read).
fn parse_vector_key_group(reader: &mut Reader) -> Result<(Vec<VectorKey>, KeyInterpolation)> {
    let num = reader.u32("num keys")? as usize;
    if num == 0 {
        return Ok((Vec::new(), KeyInterpolation::Constant));
    }
    let key_type = reader.u32("key type")?;
    let interp = KeyInterpolation::from_u32(key_type)
        .ok_or(NifError::InvalidData("unknown key interpolation type"))?;
    let mut keys = Vec::with_capacity(num.min(1024));
    let vec3 = |reader: &mut Reader, what: &'static str| -> Result<[f32; 3]> {
        Ok([reader.f32(what)?, reader.f32(what)?, reader.f32(what)?])
    };
    for _ in 0..num {
        let key = match key_type {
            KEY_CONSTANT => VectorKey {
                time: 0.0,
                value: vec3(reader, "constant key value")?,
                fwd_tangent: [0.0; 3],
                bwd_tangent: [0.0; 3],
            },
            KEY_LINEAR => VectorKey {
                time: reader.f32("linear key time")?,
                value: vec3(reader, "linear key value")?,
                fwd_tangent: [0.0; 3],
                bwd_tangent: [0.0; 3],
            },
            KEY_QUADRATIC => VectorKey {
                time: reader.f32("quadratic key time")?,
                value: vec3(reader, "quadratic key value")?,
                // File order is backward tangent FIRST.
                bwd_tangent: vec3(reader, "quadratic key bwd tangent")?,
                fwd_tangent: vec3(reader, "quadratic key fwd tangent")?,
            },
            _ => return Err(NifError::InvalidData("unknown key interpolation type")),
        };
        keys.push(key);
    }
    Ok((keys, interp))
}

/// Skip one translation (vec3 values) or scale (float values) key
/// group: u32 numKeys, then — only when the count is nonzero —
/// u32 key type and the keys. A zero count carries no type field
/// (proven by the door files: both counts 0, exactly 8 bytes for
/// both groups, 136/136 consumed). The values are not kept: doors
/// only rotate.
fn skip_key_group(reader: &mut Reader, value_floats: usize) -> Result<()> {
    let num = reader.u32("num keys")? as usize;
    if num == 0 {
        return Ok(());
    }
    let key_type = reader.u32("key type")?;
    let floats_per_key = match key_type {
        KEY_CONSTANT => value_floats,
        KEY_LINEAR => 1 + value_floats,
        KEY_QUADRATIC => 1 + 3 * value_floats,
        _ => return Err(NifError::InvalidData("unknown key interpolation type")),
    };
    let bytes = num
        .checked_mul(floats_per_key)
        .and_then(|f| f.checked_mul(4))
        .ok_or(NifError::InvalidData("key group too large"))?;
    reader.take(bytes, "skipped keys")?;
    Ok(())
}

/// Parse a `NiTransformData` block (M15): XYZ rotation keys +
/// translation keys. Layout (proven byte-exact by the `door_anim`
/// probe on real door NIFs): u32 num-rotation-keys (1, a
/// placeholder for XYZ), u32 rotation type (must be 4 = XYZ —
/// anything else errors and the caller degrades the block to
/// Unknown), three float key groups (X, Y, Z), then the
/// translation (vec3) key group, then the scale (float) key
/// group, skipped. The parse must consume the block exactly:
/// leftover bytes mean the layout drifted, and the block
/// degrades to Unknown rather than returning wrong keys.
///
/// Rotation type 1 is LINEAR quaternion keys (proven by exact
/// byte consumption on 10+ blocks in `MillLogPile.nif` — the
/// log-tumbling sequences — by `decode_transform_data.py`):
/// each key is time + quat (w, x, y, z), 5 floats, with NO
/// key-type field, and a zero num-rotation-keys count carries
/// NO rotation-type field at all (the translation group follows
/// immediately). Type 1 is NOT parsed here yet — the sequence
/// player milestone will add it, with the layout above as the
/// spec; until then type-1 blocks degrade to Unknown.
fn parse_transform_data(reader: &mut Reader) -> Result<TransformDataBlock> {
    let _num_rotation_keys = reader.u32("num rotation keys")?;
    let rotation_type = reader.u32("rotation type")?;
    if rotation_type != ROTATION_XYZ {
        return Err(NifError::InvalidData(
            "NiTransformData rotation is not XYZ Euler",
        ));
    }
    let (x_keys, x_key_type) = parse_float_key_group(reader)?;
    let (y_keys, y_key_type) = parse_float_key_group(reader)?;
    let (z_keys, z_key_type) = parse_float_key_group(reader)?;
    let (translation_keys, translation_key_type) = parse_vector_key_group(reader)?;
    skip_key_group(reader, 1)?;
    if reader.remaining() != 0 {
        return Err(NifError::InvalidData("NiTransformData has trailing bytes"));
    }
    Ok(TransformDataBlock {
        x_keys,
        y_keys,
        z_keys,
        x_key_type,
        y_key_type,
        z_key_type,
        translation_keys,
        translation_key_type,
    })
}

/// Parse a `NiControllerSequence` block (secret door). Layout
/// (proven byte-exact on RifRmSmSecretCabinetDoor01.nif and
/// FarmhouseLDoor01.nif — Open and Close, 4 sequences, all fit;
/// the nif.xml field order does not match what the files carry):
/// string name, u32 num-controlled, u32 unknown (1 in both door
/// files), then num-controlled ControllerLink blocks of 29 bytes
/// each, then a 34-byte tail (weight f32, text-keys ref, cycle
/// type u32, frequency/start/stop f32s, two u32s, two bytes).
///
/// A ControllerLink is 29 bytes: i32 interpolator ref, string
/// controller type ("NiTransformController", skipped), u8
/// unknown (0, skipped), string target node name, i32 string
/// palette ref (-1 — embedded NIFs carry no palette, skipped),
/// string (controller type again, skipped), i32 (-1, skipped),
/// i32 (-1, skipped). The tail is not parsed — only the name +
/// controlled blocks are needed — so there is no
/// exact-consumption check: the links region is validated by the
/// size-bounded reader (a truncated file errors) and the string
/// indices must resolve (garbage errors). The caller degrades
/// failures to Unknown.
fn parse_controller_sequence(
    reader: &mut Reader,
    strings: &[String],
) -> Result<ControllerSequenceBlock> {
    let name = reader.string(strings, "sequence name")?;
    let num_controlled = reader.u32("num controlled")? as usize;
    let _unknown = reader.u32("sequence unknown")?;
    let mut controlled = Vec::with_capacity(num_controlled.min(1024));
    for _ in 0..num_controlled {
        let interpolator = reader.i32("controlled interpolator ref")?;
        let _controller_type = reader.string(strings, "controlled controller type")?;
        let _unknown_byte = reader.u8("controlled unknown byte")?;
        let target_name = reader.string(strings, "controlled target name")?;
        let _string_palette = reader.i32("controlled string palette ref")?;
        let _controller_type_again = reader.string(strings, "controlled controller type 2")?;
        let _unknown1 = reader.i32("controlled unknown 1")?;
        let _unknown2 = reader.i32("controlled unknown 2")?;
        controlled.push(ControlledBlock {
            interpolator,
            target_name,
        });
    }
    Ok(ControllerSequenceBlock { name, controlled })
}

/// Parse a `NiTransformInterpolator` block (secret door): 36
/// bytes — translation (3×f32), rotation quaternion (4×f32),
/// scale (f32), then the i32 data ref at offset 32. The
/// transform fields read as -FLT_MAX (the "unset" sentinel) when
/// the data drives the transform — only the data ref is kept.
/// Proven 36/36 on 6 interpolators across 4 door NIFs by the
/// `secret_door_probe`; the parse must consume the block
/// exactly, like `NiTransformData`.
fn parse_transform_interpolator(reader: &mut Reader) -> Result<TransformInterpolatorBlock> {
    let _transform = reader.take(32, "interpolator transform")?;
    let data = reader.i32("interpolator data ref")?;
    if reader.remaining() != 0 {
        return Err(NifError::InvalidData(
            "NiTransformInterpolator has trailing bytes",
        ));
    }
    Ok(TransformInterpolatorBlock { data })
}

/// Parse a `NiTransformController` (sawmill): i32 next, u16
/// flags, f32 frequency, f32 phase, f32 start, f32 stop, i32
/// target, i32 interpolator — 30 bytes exactly. The target ref
/// always points back at the node that owns the controller (8
/// controllers in `MillLogPile.nif` +
/// `BlacksmithSharpeningWheelAnimating.nif` agree). Trailing
/// bytes mean the layout drifted; the block degrades to Unknown.
fn parse_transform_controller(reader: &mut Reader) -> Result<TransformControllerBlock> {
    let _next = reader.i32("controller next")?;
    let flags = reader.u16("controller flags")?;
    let frequency = reader.f32("controller frequency")?;
    let phase = reader.f32("controller phase")?;
    let start_time = reader.f32("controller start")?;
    let stop_time = reader.f32("controller stop")?;
    let target = reader.i32("controller target")?;
    let interpolator = reader.i32("controller interpolator")?;
    if reader.remaining() != 0 {
        return Err(NifError::InvalidData(
            "NiTransformController has trailing bytes",
        ));
    }
    Ok(TransformControllerBlock {
        flags,
        frequency,
        phase,
        start_time,
        stop_time,
        target,
        interpolator,
    })
}

/// Parse a `NiSkinInstance` / `BSDismemberSkinInstance` (Skyrim SE
/// layout): Data, SkinPartition, Root, NumBones, Bones, then — for
/// dismember only — NumPartitions and (flags, part id) pairs.
/// Inherits `NiObject`: no ObjectNET header. The partition ref
/// between Data and Root is the SE addition; old docs showing
/// Data/Root/Bones are wrong for SE.
fn parse_skin_instance(reader: &mut Reader, dismember: bool) -> Result<SkinInstanceBlock> {
    let data = reader.i32("skin data ref")?;
    let partition = reader.i32("skin partition ref")?;
    let root = reader.i32("skin root ref")?;
    let num_bones = reader.u32("num skin bones")? as usize;
    let mut bones = Vec::with_capacity(num_bones);
    for _ in 0..num_bones {
        bones.push(reader.i32("skin bone ref")?);
    }
    let mut dismember_partitions = Vec::new();
    if dismember {
        let num_partitions = reader.u32("num dismember partitions")? as usize;
        for _ in 0..num_partitions {
            let flags = reader.u16("dismember partition flags")?;
            let part_id = reader.u16("dismember part id")?;
            dismember_partitions.push((flags, part_id));
        }
    }
    Ok(SkinInstanceBlock {
        data,
        partition,
        root,
        bones,
        dismember,
        dismember_partitions,
    })
}

/// A `SkinTransform`: rotation, translation, scale — in that
/// order, unlike the AvObject layout (translation first).
fn parse_skin_transform(reader: &mut Reader) -> Result<Transform> {
    let rotation = reader.mat3("skin transform rotation")?;
    let translation = reader.vec3("skin transform translation")?;
    let scale = reader.f32("skin transform scale")?;
    Ok(Transform {
        translation,
        rotation,
        scale,
    })
}

/// Parse a `NiSkinData` (Skyrim SE layout): skin transform, bone
/// count, ONE byte (0x01 in every file checked), then one bone
/// each: skin transform, bounding sphere, vertex count, weights.
/// NO trailing skin-partition ref — the block ends after the last
/// bone's weights.
fn parse_skin_data(reader: &mut Reader) -> Result<SkinDataBlock> {
    let skin_transform = parse_skin_transform(reader)?;
    let num_bones = reader.u32("num skin data bones")? as usize;
    let unknown_byte = reader.u8("skin data unknown byte")?;
    let mut bones = Vec::with_capacity(num_bones);
    for _ in 0..num_bones {
        let transform = parse_skin_transform(reader)?;
        let center = reader.vec3("skin bone bounding sphere center")?;
        let radius = reader.f32("skin bone bounding sphere radius")?;
        let num_weights = reader.u16("num skin bone weights")? as usize;
        let mut weights = Vec::with_capacity(num_weights);
        for _ in 0..num_weights {
            let index = reader.u16("skin weight vertex index")?;
            let weight = reader.f32("skin weight")?;
            weights.push((index, weight));
        }
        bones.push(SkinBone {
            transform,
            bounding_sphere: (center, radius),
            weights,
        });
    }
    Ok(SkinDataBlock {
        skin_transform,
        unknown_byte,
        bones,
        trailing_bytes: reader.remaining(),
    })
}

/// Parse a `NiSkinPartition` (Skyrim SE layout, see
/// [`SkinPartitionBlock`]): the shape's full vertex array first,
/// then one partition struct per dismember part, each followed by
/// a repeated vertex desc and a duplicate triangle list (parsed
/// once, duplicate skipped). Strips are triangulated the same way
/// as `NiTriStripsData`.
fn parse_skin_partition(reader: &mut Reader) -> Result<SkinPartitionBlock> {
    let num_partitions = reader.u32("num skin partitions")? as usize;
    let data_size = reader.u32("skin vertex data size")? as usize;
    let vertex_size = reader.u32("skin vertex size")? as usize;
    let vertex_desc = reader.u64("skin vertex desc")?;
    let arg = (vertex_desc >> 44) as u16;
    let num_vertices = data_size.checked_div(vertex_size).unwrap_or(0);
    let mut vertices = Vec::with_capacity(num_vertices);
    for _ in 0..num_vertices {
        vertices.push(parse_bs_vertex(reader, arg)?);
    }
    let mut partitions = Vec::with_capacity(num_partitions);
    for _ in 0..num_partitions {
        let num_vertices = reader.u16("skin partition num vertices")? as usize;
        let num_triangles = reader.u16("skin partition num triangles")? as usize;
        let num_bones = reader.u16("skin partition num bones")? as usize;
        let num_strips = reader.u16("skin partition num strips")? as usize;
        let num_weights = reader.u16("skin partition num weights per vertex")? as usize;
        let mut bones = Vec::with_capacity(num_bones);
        for _ in 0..num_bones {
            bones.push(reader.u16("skin partition bone")?);
        }
        let has_vertex_map = reader.bool("skin partition has vertex map")?;
        let mut vertex_map = Vec::with_capacity(num_vertices);
        if has_vertex_map {
            for _ in 0..num_vertices {
                vertex_map.push(reader.u16("skin partition vertex map")?);
            }
        } else {
            // No map stored: partition-local == global.
            vertex_map = (0..num_vertices as u16).collect();
        }
        let has_vertex_weights = reader.bool("skin partition has vertex weights")?;
        let mut vertex_weights: Vec<Vec<f32>> = Vec::with_capacity(num_vertices);
        if has_vertex_weights {
            for _ in 0..num_vertices {
                let mut row = Vec::with_capacity(num_weights);
                for _ in 0..num_weights {
                    row.push(reader.f32("skin partition vertex weight")?);
                }
                vertex_weights.push(row);
            }
        }
        let mut strip_lengths = Vec::with_capacity(num_strips);
        for _ in 0..num_strips {
            strip_lengths.push(reader.u16("skin partition strip length")? as usize);
        }
        let has_faces = reader.bool("skin partition has faces")?;
        let mut triangles: Vec<[u16; 3]> = Vec::new();
        if has_faces {
            if num_strips > 0 {
                for len in &strip_lengths {
                    let mut strip = Vec::with_capacity(*len);
                    for _ in 0..*len {
                        strip.push(reader.u16("skin partition strip index")?);
                    }
                    triangles.extend(triangulate_strip(&strip));
                }
            } else {
                for _ in 0..num_triangles {
                    triangles.push([
                        reader.u16("skin partition triangle")?,
                        reader.u16("skin partition triangle")?,
                        reader.u16("skin partition triangle")?,
                    ]);
                }
            }
        }
        let has_bone_indices = reader.bool("skin partition has bone indices")?;
        let mut bone_indices: Vec<Vec<u8>> = Vec::with_capacity(num_vertices);
        if has_bone_indices {
            for _ in 0..num_vertices {
                bone_indices.push(
                    reader
                        .take(num_weights, "skin partition bone indices")?
                        .to_vec(),
                );
            }
        }
        let _unknown_short = reader.u16("skin partition unknown short")?;
        // Trailer: the header vertex desc repeated, then the
        // triangle list duplicated. Both are skipped — the struct's
        // copy above is authoritative. (The trailer is present in
        // every file checked; the sweep reports any file where the
        // parse ends with trailing bytes instead.)
        let _dup_desc = reader.u64("skin partition duplicate vertex desc")?;
        if has_faces {
            if num_strips > 0 {
                for len in &strip_lengths {
                    reader.take(len * 2, "skin partition duplicate strip")?;
                }
            } else {
                reader.take(num_triangles * 6, "skin partition duplicate triangles")?;
            }
        }
        // Resolve weights to skin-instance bone indices now, so the
        // assembler never touches partition-local slots.
        let mut weights: Vec<Vec<(u16, f32)>> = Vec::with_capacity(num_vertices);
        for local in 0..num_vertices {
            let mut row = Vec::new();
            if has_vertex_weights && has_bone_indices {
                for k in 0..num_weights {
                    let weight = vertex_weights[local][k];
                    if weight > 0.0 {
                        let slot = bone_indices[local][k] as usize;
                        if let Some(&bone) = bones.get(slot) {
                            row.push((bone, weight));
                        }
                    }
                }
            }
            weights.push(row);
        }
        partitions.push(SkinPartitionData {
            bones,
            vertex_map,
            triangles,
            weights,
        });
    }
    Ok(SkinPartitionBlock {
        vertex_desc,
        vertices,
        partitions,
        trailing_bytes: reader.remaining(),
    })
}

/// Parse `NiObjectNET`. For `BSLightingShaderProperty` the shader
/// type u32 comes first, before the name; the returned u32 is the
/// shader type (0 for other blocks).
fn parse_object_net(
    reader: &mut Reader,
    strings: &[String],
    lighting_shader: bool,
) -> Result<(ObjectNet, u32)> {
    let shader_type = if lighting_shader {
        reader.u32("shader type")?
    } else {
        0
    };
    let name = reader.string(strings, "object name")?;
    let num_extra = reader.u32("num extra data")? as usize;
    let mut extra_data = Vec::with_capacity(num_extra);
    for _ in 0..num_extra {
        extra_data.push(reader.i32("extra data ref")?);
    }
    let controller = reader.i32("controller ref")?;
    Ok((
        ObjectNet {
            name,
            extra_data,
            controller,
        },
        shader_type,
    ))
}

fn parse_av_object(reader: &mut Reader, strings: &[String]) -> Result<AvObject> {
    let (net, _) = parse_object_net(reader, strings, false)?;
    // Flags is a u32 for BS version > 26 (Skyrim LE/SE).
    let flags = reader.u32("AV flags")?;
    let translation = reader.vec3("translation")?;
    let rotation = reader.mat3("rotation")?;
    let scale = reader.f32("scale")?;
    // Num Properties / Properties exist only for BS version <= 34
    // (Oblivion and earlier); Skyrim has no property list here.
    let collision_object = reader.i32("collision object ref")?;
    Ok(AvObject {
        net,
        flags,
        translation,
        rotation,
        scale,
        collision_object,
    })
}

fn parse_material_data(reader: &mut Reader, strings: &[String]) -> Result<MaterialData> {
    let num = reader.u32("num materials")? as usize;
    let mut names = Vec::with_capacity(num);
    for _ in 0..num {
        names.push(reader.string(strings, "material name")?);
    }
    let mut extra_data = Vec::with_capacity(num);
    for _ in 0..num {
        extra_data.push(reader.i32("material extra data")?);
    }
    let active_material = reader.i32("active material")?;
    let needs_update = reader.bool("material needs update")?;
    Ok(MaterialData {
        names,
        extra_data,
        active_material,
        needs_update,
    })
}

fn parse_geometry(reader: &mut Reader, strings: &[String]) -> Result<Geometry> {
    let av = parse_av_object(reader, strings)?;
    let data = reader.i32("geometry data ref")?;
    let skin_instance = reader.i32("skin instance ref")?;
    let material_data = parse_material_data(reader, strings)?;
    let shader_property = reader.i32("shader property ref")?;
    let alpha_property = reader.i32("alpha property ref")?;
    Ok(Geometry {
        av,
        data,
        skin_instance,
        material_data,
        shader_property,
        alpha_property,
    })
}

fn parse_node(reader: &mut Reader, strings: &[String]) -> Result<NodeBlock> {
    let av = parse_av_object(reader, strings)?;
    let num_children = reader.u32("num children")? as usize;
    let mut children = Vec::with_capacity(num_children);
    for _ in 0..num_children {
        children.push(reader.i32("child ref")?);
    }
    // Num Effects / Effects exist for BS version < 130 (Skyrim).
    let num_effects = reader.u32("num effects")? as usize;
    let mut effects = Vec::with_capacity(num_effects);
    for _ in 0..num_effects {
        effects.push(reader.i32("effect ref")?);
    }
    Ok(NodeBlock {
        av,
        children,
        effects,
    })
}

fn parse_geometry_data(reader: &mut Reader, _strings: &[String]) -> Result<GeometryData> {
    let group_id = reader.i32("group ID")?;
    let num_vertices = reader.u16("num vertices")? as usize;
    let keep_flags = reader.u8("keep flags")?;
    let compress_flags = reader.u8("compress flags")?;
    let has_vertices = reader.bool("has vertices")?;
    let mut vertices = Vec::new();
    if has_vertices {
        vertices.reserve(num_vertices);
        for _ in 0..num_vertices {
            vertices.push(reader.vec3("vertex")?);
        }
    }
    let bs_data_flags = reader.u16("BS data flags")?;
    let material_crc = reader.u32("material CRC")?;
    let has_normals = reader.bool("has normals")?;
    let mut normals = Vec::new();
    if has_normals {
        normals.reserve(num_vertices);
        for _ in 0..num_vertices {
            normals.push(reader.vec3("normal")?);
        }
    }
    let has_tangents = has_normals && bs_data_flags & 0x1000 != 0;
    let mut tangents = Vec::new();
    let mut bitangents = Vec::new();
    if has_tangents {
        tangents.reserve(num_vertices);
        bitangents.reserve(num_vertices);
        for _ in 0..num_vertices {
            tangents.push(reader.vec3("tangent")?);
        }
        for _ in 0..num_vertices {
            bitangents.push(reader.vec3("bitangent")?);
        }
    }
    let bounding_sphere = (
        reader.vec3("bounding sphere center")?,
        reader.f32("bounding sphere radius")?,
    );
    let has_vertex_colors = reader.bool("has vertex colors")?;
    let mut vertex_colors = Vec::new();
    if has_vertex_colors {
        vertex_colors.reserve(num_vertices);
        for _ in 0..num_vertices {
            vertex_colors.push(reader.vec4("vertex color")?);
        }
    }
    // UV set count is BS Data Flags bit 0: 0 or 1 set for Skyrim,
    // stored set-major (each set is Num Vertices coords).
    let mut uvs = Vec::new();
    if bs_data_flags & 1 != 0 {
        uvs.reserve(num_vertices);
        for _ in 0..num_vertices {
            uvs.push(reader.vec2("UV")?);
        }
    }
    let consistency_flags = reader.u16("consistency flags")?;
    let additional_data = reader.i32("additional data ref")?;
    Ok(GeometryData {
        group_id,
        keep_flags,
        compress_flags,
        vertices,
        bs_data_flags,
        material_crc,
        normals,
        tangents,
        bitangents,
        bounding_sphere,
        vertex_colors,
        uvs,
        consistency_flags,
        additional_data,
    })
}

fn parse_tri_shape_data(reader: &mut Reader, strings: &[String]) -> Result<TriShapeDataBlock> {
    let data = parse_geometry_data(reader, strings)?;
    // NiTriBasedGeomData adds Num Triangles.
    let num_triangles = reader.u16("num triangles")? as usize;
    let _num_triangle_points = reader.u32("num triangle points")?;
    let has_triangles = reader.bool("has triangles")?;
    let mut triangles = Vec::new();
    if has_triangles {
        triangles.reserve(num_triangles);
        for _ in 0..num_triangles {
            triangles.push(reader.triangle("triangle")?);
        }
    }
    let num_match_groups = reader.u16("num match groups")? as usize;
    let mut match_groups = Vec::with_capacity(num_match_groups);
    for _ in 0..num_match_groups {
        let count = reader.u16("match group size")? as usize;
        let mut group = Vec::with_capacity(count);
        for _ in 0..count {
            group.push(reader.u16("match group vertex")?);
        }
        match_groups.push(group);
    }
    Ok(TriShapeDataBlock {
        data,
        num_triangles: num_triangles as u16,
        triangles,
        match_groups,
    })
}

fn parse_tri_strips_data(reader: &mut Reader, strings: &[String]) -> Result<TriStripsDataBlock> {
    let data = parse_geometry_data(reader, strings)?;
    let num_triangles = reader.u16("num triangles")?;
    let num_strips = reader.u16("num strips")? as usize;
    let mut strip_lengths = Vec::with_capacity(num_strips);
    for _ in 0..num_strips {
        strip_lengths.push(reader.u16("strip length")? as usize);
    }
    let has_points = reader.bool("has points")?;
    let mut strips = Vec::with_capacity(num_strips);
    for length in &strip_lengths {
        let mut strip = Vec::with_capacity(*length);
        if has_points {
            for _ in 0..*length {
                strip.push(reader.u16("strip point")?);
            }
        }
        strips.push(strip);
    }
    Ok(TriStripsDataBlock {
        data,
        num_triangles,
        strips,
    })
}

/// Parse one `BSVertexDataSSE` vertex. `arg` is the attribute bits
/// (Vertex Desc >> 44).
fn parse_bs_vertex(reader: &mut Reader, arg: u16) -> Result<BsVertex> {
    let mut vertex = BsVertex::default();
    if arg & 0x1 != 0 {
        vertex.position = Some(reader.vec3("BS vertex position")?);
        if arg & 0x10 != 0 {
            vertex.bitangent_x = Some(reader.f32("BS bitangent X")?);
        } else {
            let _unused_w = reader.u32("BS unused W")?;
        }
    }
    if arg & 0x2 != 0 {
        vertex.uv = Some(reader.half_tex_coord("BS vertex UV")?);
    }
    if arg & 0x8 != 0 {
        vertex.normal = Some(reader.byte_vec3("BS vertex normal")?);
        vertex.bitangent_y = Some(reader.normbyte("BS bitangent Y")?);
    }
    if arg & 0x18 == 0x18 {
        vertex.tangent = Some(reader.byte_vec3("BS vertex tangent")?);
        vertex.bitangent_z = Some(reader.normbyte("BS bitangent Z")?);
    }
    if arg & 0x20 != 0 {
        let bytes = reader.take(4, "BS vertex color")?;
        vertex.color = Some(bytes.try_into().unwrap());
    }
    if arg & 0x40 != 0 {
        // Bone weights + indices, stored inline in the vertex
        // record. The skin partition's per-partition weights are
        // authoritative for the bind-pose reconstruction (M12), so
        // these are read only to walk the stream.
        for _ in 0..4 {
            let _weight = reader.hfloat("BS bone weight")?;
        }
        let _indices = reader.take(4, "BS bone indices")?;
    }
    if arg & 0x100 != 0 {
        vertex.eye_data = Some(reader.f32("BS eye data")?);
    }
    Ok(vertex)
}

fn parse_bs_tri_shape(reader: &mut Reader, strings: &[String]) -> Result<BsTriShapeBlock> {
    let av = parse_av_object(reader, strings)?;
    let bounding_sphere = (
        reader.vec3("bounding sphere center")?,
        reader.f32("bounding sphere radius")?,
    );
    let skin = reader.i32("skin ref")?;
    let shader_property = reader.i32("shader property ref")?;
    let alpha_property = reader.i32("alpha property ref")?;
    let vertex_desc = reader.u64("vertex desc")?;
    let num_triangles = reader.u16("num triangles")? as usize;
    let num_vertices = reader.u16("num vertices")? as usize;
    let data_size = reader.u32("data size")?;
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    if data_size > 0 {
        let arg = (vertex_desc >> 44) as u16;
        vertices.reserve(num_vertices);
        for _ in 0..num_vertices {
            vertices.push(parse_bs_vertex(reader, arg)?);
        }
        triangles.reserve(num_triangles);
        for _ in 0..num_triangles {
            triangles.push(reader.triangle("triangle")?);
        }
    }
    // Half-precision particle copy (SE only).
    let particle_data_size = reader.u32("particle data size")?;
    let mut particle_vertices = Vec::new();
    let mut particle_normals = Vec::new();
    let mut particle_triangles = Vec::new();
    if particle_data_size > 0 {
        particle_vertices.reserve(num_vertices);
        particle_normals.reserve(num_vertices);
        for _ in 0..num_vertices {
            particle_vertices.push(reader.half_vec3("particle vertex")?);
        }
        for _ in 0..num_vertices {
            particle_normals.push(reader.half_vec3("particle normal")?);
        }
        particle_triangles.reserve(num_triangles);
        for _ in 0..num_triangles {
            particle_triangles.push(reader.triangle("particle triangle")?);
        }
    }
    Ok(BsTriShapeBlock {
        av,
        bounding_sphere,
        skin,
        shader_property,
        alpha_property,
        vertex_desc,
        num_triangles: num_triangles as u16,
        num_vertices: num_vertices as u16,
        vertices,
        triangles,
        particle_vertices,
        particle_normals,
        particle_triangles,
    })
}

/// Parse the dynamic section of a `BSDynamicTriShape`: a u32 data
/// size followed by that many bytes of dynamic vertex records. Each
/// record is `stride` bytes (vertex desc bits 4-7, × 4); the first
/// 12 bytes are the vertex position, the rest of the record is
/// uninterpreted and skipped. Absent when fewer than 4 bytes remain.
/// Returns (positions, declared data size).
fn parse_dynamic_data(reader: &mut Reader, shape: &BsTriShapeBlock) -> Result<(Vec<Vec3>, u32)> {
    if reader.remaining() < 4 {
        return Ok((Vec::new(), 0));
    }
    let data_size = reader.u32("dynamic data size")?;
    let bytes = reader.take(data_size as usize, "dynamic data")?;
    let stride = (((shape.vertex_desc >> 4) & 0xF) * 4) as usize;
    if stride < 12 || shape.num_vertices == 0 {
        return Ok((Vec::new(), data_size));
    }
    let mut positions = Vec::new();
    let records = (bytes.len() / stride).min(shape.num_vertices as usize);
    for i in 0..records {
        let base = i * stride;
        let f =
            |off: usize| f32::from_le_bytes(bytes[base + off..base + off + 4].try_into().unwrap());
        positions.push([f(0), f(4), f(8)]);
    }
    Ok((positions, data_size))
}

fn parse_tex_desc(reader: &mut Reader) -> Result<TexDesc> {
    let source = reader.i32("texture source ref")?;
    let flags = reader.u16("texture map flags")?;
    let has_transform = reader.bool("has texture transform")?;
    let transform = if has_transform {
        Some(TexTransform {
            translation: reader.vec2("texture translation")?,
            scale: reader.vec2("texture scale")?,
            rotation: reader.f32("texture rotation")?,
            method: reader.u32("texture transform method")?,
            center: reader.vec2("texture transform center")?,
        })
    } else {
        None
    };
    Ok(TexDesc {
        source,
        flags,
        transform,
    })
}

fn parse_shader_tex_desc(reader: &mut Reader) -> Result<ShaderTexDesc> {
    let has_map = reader.bool("has shader map")?;
    let (map, map_id) = if has_map {
        (
            Some(parse_tex_desc(reader)?),
            Some(reader.u32("shader map ID")?),
        )
    } else {
        (None, None)
    };
    Ok(ShaderTexDesc { map, map_id })
}

/// Parse one optional texture slot: a bool, then a `TexDesc` when
/// set. Used for the slots that are always present in the layout.
fn parse_optional_tex_desc(reader: &mut Reader, what: &'static str) -> Result<Option<TexDesc>> {
    if reader.bool(what)? {
        Ok(Some(parse_tex_desc(reader)?))
    } else {
        Ok(None)
    }
}

fn parse_texturing_property(
    reader: &mut Reader,
    strings: &[String],
) -> Result<TexturingPropertyBlock> {
    let (net, _) = parse_object_net(reader, strings, false)?;
    let flags = reader.u16("texturing flags")?;
    let texture_count = reader.u32("texture count")?;
    let base = parse_optional_tex_desc(reader, "has base texture")?;
    let dark = parse_optional_tex_desc(reader, "has dark texture")?;
    let detail = parse_optional_tex_desc(reader, "has detail texture")?;
    let gloss = parse_optional_tex_desc(reader, "has gloss texture")?;
    let glow = parse_optional_tex_desc(reader, "has glow texture")?;
    // Bump map: present in the layout only when Texture Count > 5.
    let bump_map = if texture_count > 5 && reader.bool("has bump map texture")? {
        Some(BumpMap {
            map: parse_tex_desc(reader)?,
            luma_scale: reader.f32("bump map luma scale")?,
            luma_offset: reader.f32("bump map luma offset")?,
            matrix: [
                reader.f32("bump map matrix")?,
                reader.f32("bump map matrix")?,
                reader.f32("bump map matrix")?,
                reader.f32("bump map matrix")?,
            ],
        })
    } else {
        None
    };
    // Normal: only when Texture Count > 6.
    let normal = if texture_count > 6 {
        parse_optional_tex_desc(reader, "has normal texture")?
    } else {
        None
    };
    // Parallax: only when Texture Count > 7.
    let parallax = if texture_count > 7 && reader.bool("has parallax texture")? {
        Some(ParallaxMap {
            map: parse_tex_desc(reader)?,
            offset: reader.f32("parallax offset")?,
        })
    } else {
        None
    };
    // Decals 0-3: only when Texture Count > 8/9/10/11.
    let mut decals: [Option<TexDesc>; 4] = [None, None, None, None];
    for (i, slot) in decals.iter_mut().enumerate() {
        if texture_count > 8 + i as u32 {
            *slot = parse_optional_tex_desc(reader, "has decal texture")?;
        }
    }
    let num_shader_textures = reader.u32("num shader textures")? as usize;
    let mut shader_textures = Vec::with_capacity(num_shader_textures);
    for _ in 0..num_shader_textures {
        shader_textures.push(parse_shader_tex_desc(reader)?);
    }
    Ok(TexturingPropertyBlock {
        net,
        flags,
        texture_count,
        base,
        dark,
        detail,
        gloss,
        glow,
        bump_map,
        normal,
        parallax,
        decals,
        shader_textures,
    })
}

fn parse_source_texture(reader: &mut Reader, strings: &[String]) -> Result<SourceTextureBlock> {
    let (net, _) = parse_object_net(reader, strings, false)?;
    let use_external = reader.u8("use external")?;
    // File Name is present for 20.2.0.7 either way: the texture path
    // when external, the original source filename when embedded.
    let file_name = reader.string(strings, "texture file name")?;
    let pixel_data = reader.i32("pixel data ref")?;
    let format_prefs = FormatPrefs {
        pixel_layout: reader.u32("pixel layout")?,
        use_mipmaps: reader.u32("use mipmaps")?,
        alpha_format: reader.u32("alpha format")?,
    };
    let is_static = reader.u8("is static")?;
    let direct_render = reader.bool("direct render")?;
    let persist_render_data = reader.bool("persist render data")?;
    Ok(SourceTextureBlock {
        net,
        use_external,
        file_name,
        pixel_data,
        format_prefs,
        is_static,
        direct_render,
        persist_render_data,
    })
}

fn parse_lighting_shader_property(
    reader: &mut Reader,
    strings: &[String],
) -> Result<LightingShaderPropertyBlock> {
    let (net, shader_type) = parse_object_net(reader, strings, true)?;
    // BSShaderProperty and NiShadeProperty add no fields for Skyrim
    // (their fields exist only for BS version <= 34).
    let shader_flags_1 = reader.u32("shader flags 1")?;
    let shader_flags_2 = reader.u32("shader flags 2")?;
    let uv_offset = reader.vec2("UV offset")?;
    let uv_scale = reader.vec2("UV scale")?;
    let texture_set = reader.i32("texture set ref")?;
    let emissive_color = reader.vec3("emissive color")?;
    let emissive_multiple = reader.f32("emissive multiple")?;
    let texture_clamp_mode = reader.u32("texture clamp mode")?;
    let alpha = reader.f32("alpha")?;
    let refraction_strength = reader.f32("refraction strength")?;
    let glossiness = reader.f32("glossiness")?;
    let specular_color = reader.vec3("specular color")?;
    let specular_strength = reader.f32("specular strength")?;
    let lighting_effect_1 = reader.f32("lighting effect 1")?;
    let lighting_effect_2 = reader.f32("lighting effect 2")?;
    let extra = match shader_type {
        1 => ShaderTypeExtra::EnvironmentMap {
            env_map_scale: reader.f32("environment map scale")?,
        },
        5 => ShaderTypeExtra::SkinTint {
            color: reader.vec3("skin tint color")?,
        },
        6 => ShaderTypeExtra::HairTint {
            color: reader.vec3("hair tint color")?,
        },
        7 => ShaderTypeExtra::ParallaxOcc {
            max_passes: reader.f32("max passes")?,
            scale: reader.f32("parallax scale")?,
        },
        11 => ShaderTypeExtra::MultiLayerParallax {
            inner_layer_thickness: reader.f32("parallax inner layer thickness")?,
            refraction_scale: reader.f32("parallax refraction scale")?,
            inner_layer_texture_scale: reader.vec2("parallax inner layer texture scale")?,
            envmap_strength: reader.f32("parallax envmap strength")?,
        },
        14 => ShaderTypeExtra::SparkleSnow {
            sparkle_parameters: reader.vec4("sparkle parameters")?,
        },
        16 => ShaderTypeExtra::EyeEnvmap {
            cubemap_scale: reader.f32("eye cubemap scale")?,
            left_eye_reflection_center: reader.vec3("left eye reflection center")?,
            right_eye_reflection_center: reader.vec3("right eye reflection center")?,
        },
        _ => ShaderTypeExtra::None,
    };
    Ok(LightingShaderPropertyBlock {
        net,
        shader_type,
        shader_flags_1,
        shader_flags_2,
        uv_offset,
        uv_scale,
        texture_set,
        emissive_color,
        emissive_multiple,
        texture_clamp_mode,
        alpha,
        refraction_strength,
        glossiness,
        specular_color,
        specular_strength,
        lighting_effect_1,
        lighting_effect_2,
        extra,
    })
}

fn parse_shader_texture_set(reader: &mut Reader) -> Result<ShaderTextureSetBlock> {
    let num_textures = reader.u32("num textures")? as usize;
    let mut textures = Vec::with_capacity(num_textures);
    for _ in 0..num_textures {
        textures.push(reader.sized_string("texture path")?);
    }
    Ok(ShaderTextureSetBlock { textures })
}

fn parse_alpha_property(reader: &mut Reader, strings: &[String]) -> Result<AlphaPropertyBlock> {
    let (net, _) = parse_object_net(reader, strings, false)?;
    let flags = reader.u16("alpha flags")?;
    let threshold = reader.u8("alpha threshold")?;
    Ok(AlphaPropertyBlock {
        net,
        flags,
        threshold,
    })
}

/// Parse a `BSEffectShaderProperty`. Layout proven byte-exact on
/// `meshes/effects/fxwaterfallskirttallfront.nif` blocks [4] (160
/// B, gradient texture present) and [22] (125 B, trailing f32
/// 0.0 instead): `NiObjectNET` (no shader-type prefix), flags 1/2,
/// UV offset/scale, source texture, u32 clamp mode, 4 unknown
/// floats, emissive Color4, 2 unknown floats, then optionally a
/// second sized string. The second string is read ONLY when its
/// length exactly fills the remaining block bytes — a trailing
/// float (block [22]'s 0.0) parses as an empty string, and a
/// foreign tail is left unread rather than failing the parse.
fn parse_effect_shader_property(
    reader: &mut Reader,
    strings: &[String],
) -> Result<EffectShaderPropertyBlock> {
    let (net, _) = parse_object_net(reader, strings, false)?;
    let shader_flags_1 = reader.u32("shader flags 1")?;
    let shader_flags_2 = reader.u32("shader flags 2")?;
    let uv_offset = reader.vec2("UV offset")?;
    let uv_scale = reader.vec2("UV scale")?;
    let source_texture = reader.sized_string("source texture")?;
    let texture_clamp_mode = reader.u32("texture clamp mode")?;
    let unknown_pre_emissive = [
        reader.f32("unknown pre-emissive 0")?,
        reader.f32("unknown pre-emissive 1")?,
        reader.f32("unknown pre-emissive 2")?,
        reader.f32("unknown pre-emissive 3")?,
    ];
    let emissive = reader.vec4("emissive color")?;
    let unknown_post_emissive = [
        reader.f32("unknown post-emissive 0")?,
        reader.f32("unknown post-emissive 1")?,
    ];
    // Optional gradient texture: present only when a sized string
    // of exactly the remaining length follows. Peek the length
    // without consuming — a foreign tail must not fail the parse.
    let mut gradient_texture = None;
    if reader.remaining() >= 4 {
        let len_bytes: [u8; 4] = reader.bytes[reader.pos..reader.pos + 4].try_into().unwrap();
        let len = u32::from_le_bytes(len_bytes) as usize;
        if 4 + len == reader.remaining() {
            let text = reader.sized_string("gradient texture")?;
            if !text.is_empty() {
                gradient_texture = Some(text);
            }
        }
    }
    Ok(EffectShaderPropertyBlock {
        net,
        shader_flags_1,
        shader_flags_2,
        uv_offset,
        uv_scale,
        source_texture,
        texture_clamp_mode,
        unknown_pre_emissive,
        emissive,
        unknown_post_emissive,
        gradient_texture,
    })
}

fn parse_material_property(
    reader: &mut Reader,
    strings: &[String],
) -> Result<MaterialPropertyBlock> {
    let (net, _) = parse_object_net(reader, strings, false)?;
    // Ambient/Diffuse exist only for BS version < 26; Skyrim starts
    // at Specular.
    let specular_color = reader.vec3("specular color")?;
    let emissive_color = reader.vec3("emissive color")?;
    let glossiness = reader.f32("glossiness")?;
    let alpha = reader.f32("alpha")?;
    let emissive_mult = reader.f32("emissive mult")?;
    Ok(MaterialPropertyBlock {
        net,
        specular_color,
        emissive_color,
        glossiness,
        alpha,
        emissive_mult,
    })
}

/// Widen a 16-bit IEEE half float to f32.
fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits >> 15) & 1;
    let exp = u32::from(bits >> 10) & 0x1f;
    let mant = u32::from(bits & 0x3ff);
    let f32_bits = match exp {
        0 => {
            if mant == 0 {
                // Signed zero.
                sign << 31
            } else {
                // Subnormal: normalize the mantissa.
                let mut m = mant;
                let mut shift = 0;
                while m & 0x400 == 0 {
                    m <<= 1;
                    shift += 1;
                }
                m &= 0x3ff;
                // Unbiased exponent is -14 - shift; f32 bias is 127.
                (sign << 31) | ((113 - shift) << 23) | (m << 13)
            }
        }
        0x1f => {
            // Infinity or NaN.
            (sign << 31) | (0xff << 23) | (mant << 13)
        }
        _ => {
            // Normalized: rebias the exponent from 15 to 127.
            (sign << 31) | ((exp + 112) << 23) | (mant << 13)
        }
    };
    f32::from_bits(f32_bits)
}

/// Decode a `normbyte`: a signed byte holding a float in [-1, 1].
fn normbyte_to_f32(value: i8) -> f32 {
    (f32::from(value) / 127.0).clamp(-1.0, 1.0)
}

/// Triangulate one triangle strip with alternating winding.
/// Degenerate triangles (any repeated index) are dropped.
fn triangulate_strip(strip: &[u16]) -> Vec<[u16; 3]> {
    let mut out = Vec::new();
    for (i, window) in strip.windows(3).enumerate() {
        let (a, b, c) = (window[0], window[1], window[2]);
        if a == b || b == c || a == c {
            continue;
        }
        if i % 2 == 0 {
            out.push([a, b, c]);
        } else {
            out.push([b, a, c]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_known_values() {
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xBC00), -1.0);
        assert_eq!(f16_to_f32(0x3800), 0.5);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x8000), -0.0);
        assert!(f16_to_f32(0x7C00).is_infinite());
        assert!(f16_to_f32(0x7E00).is_nan());
        // Smallest subnormal: 2^-24.
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        // Largest finite half: 65504.
        assert_eq!(f16_to_f32(0x7BFF), 65504.0);
    }

    #[test]
    fn normbyte_known_values() {
        assert_eq!(normbyte_to_f32(127), 1.0);
        assert_eq!(normbyte_to_f32(-127), -1.0);
        assert_eq!(normbyte_to_f32(0), 0.0);
        // -128 clamps to -1.0 rather than -1.0078...
        assert_eq!(normbyte_to_f32(-128), -1.0);
    }

    #[test]
    fn editor_geometry_names() {
        // v0.4.7: the yellow shapes / sky poles / white streaks.
        assert!(is_editor_geometry_name("EditorMarker", ""));
        assert!(is_editor_geometry_name("editormarker", ""));
        assert!(is_editor_geometry_name("emitShape", ""));
        assert!(is_editor_geometry_name("emitShape01:0", ""));
        assert!(is_editor_geometry_name("splashEmitMesh:0", ""));
        assert!(is_editor_geometry_name("Emit_LargeOuterArms01:0", ""));
        // A real shape under an EditorMarker node is editor too.
        assert!(is_editor_geometry_name("Plane01", "EditorMarker"));
        // Real geometry is untouched — including names that merely
        // contain "emit" without the emitter patterns.
        assert!(!is_editor_geometry_name("Door:7", ""));
        assert!(!is_editor_geometry_name("FarmTable01:5", ""));
        assert!(!is_editor_geometry_name("MaleHeadNord", ""));
        assert!(!is_editor_geometry_name("BIGwHITEWATER01", ""));
        assert!(!is_editor_geometry_name("Tablet01:0", "Door"));
        assert!(!is_editor_geometry_name("", ""));
    }

    #[test]
    fn strip_triangulation() {
        // A 4-point strip makes 2 triangles with flipped winding on
        // the second.
        assert_eq!(triangulate_strip(&[0, 1, 2, 3]), vec![[0, 1, 2], [2, 1, 3]]);
        // Degenerate triangles (repeated index) are dropped.
        assert!(triangulate_strip(&[0, 1, 1, 2]).is_empty());
        assert!(triangulate_strip(&[0, 1]).is_empty());
    }

    #[test]
    fn string_index_resolution() {
        let strings = vec!["alpha".to_string(), "beta".to_string()];
        assert_eq!(resolve_string(&strings, 0).unwrap(), "alpha");
        assert_eq!(resolve_string(&strings, 1).unwrap(), "beta");
        assert_eq!(resolve_string(&strings, 0xFFFF_FFFF).unwrap(), "");
        assert!(resolve_string(&strings, 2).is_err());
    }

    fn push_i32(bytes: &mut Vec<u8>, v: i32) {
        bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn push_u16(bytes: &mut Vec<u8>, v: u16) {
        bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn push_u32(bytes: &mut Vec<u8>, v: u32) {
        bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn push_u64(bytes: &mut Vec<u8>, v: u64) {
        bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn push_f32(bytes: &mut Vec<u8>, v: f32) {
        bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn push_vec3(bytes: &mut Vec<u8>, v: Vec3) {
        for c in v {
            push_f32(bytes, c);
        }
    }

    fn push_identity_mat3(bytes: &mut Vec<u8>) {
        push_vec3(bytes, [1.0, 0.0, 0.0]);
        push_vec3(bytes, [0.0, 1.0, 0.0]);
        push_vec3(bytes, [0.0, 0.0, 1.0]);
    }

    /// A SkinTransform blob: rotation, translation, scale.
    fn push_skin_transform(bytes: &mut Vec<u8>, translation: Vec3) {
        push_identity_mat3(bytes);
        push_vec3(bytes, translation);
        push_f32(bytes, 1.0);
    }

    #[test]
    fn skin_instance_se_layout() {
        // SE order: Data, SkinPartition, Root, NumBones, Bones.
        let mut bytes = Vec::new();
        push_i32(&mut bytes, 8);
        push_i32(&mut bytes, 9);
        push_i32(&mut bytes, 5);
        push_u32(&mut bytes, 2);
        push_i32(&mut bytes, 1);
        push_i32(&mut bytes, 2);
        let block = parse_block("NiSkinInstance", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        let Block::SkinInstance(inst) = block else {
            panic!("expected SkinInstance");
        };
        assert_eq!(inst.data, 8);
        assert_eq!(inst.partition, 9);
        assert_eq!(inst.root, 5);
        assert_eq!(inst.bones, vec![1, 2]);
        assert!(!inst.dismember);
        assert!(inst.dismember_partitions.is_empty());

        // Dismember variant: same fields + partitions.
        push_u32(&mut bytes, 1);
        push_u16(&mut bytes, 0x0101);
        push_u16(&mut bytes, 0x0020);
        let block =
            parse_block("BSDismemberSkinInstance", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        let Block::SkinInstance(inst) = block else {
            panic!("expected SkinInstance");
        };
        assert!(inst.dismember);
        assert_eq!(inst.dismember_partitions, vec![(0x0101, 0x0020)]);

        // LE keeps the old behavior: skin blocks stay Unknown.
        let block = parse_block("NiSkinInstance", &bytes, &[], BS_VERSION_SKYRIM_LE).unwrap();
        assert!(matches!(block, Block::Unknown { .. }));
    }

    #[test]
    fn skin_data_ends_clean() {
        // Skin transform (identity), 1 bone, the 0x01 byte, then
        // the bone: identity transform, sphere, 2 weights.
        let mut bytes = Vec::new();
        push_skin_transform(&mut bytes, [0.0, 0.0, 0.0]);
        push_u32(&mut bytes, 1);
        bytes.push(0x01);
        push_skin_transform(&mut bytes, [0.0, -1.55, 51.43]);
        push_vec3(&mut bytes, [0.0, 0.0, 0.0]);
        push_f32(&mut bytes, 10.0);
        push_u16(&mut bytes, 2);
        push_u16(&mut bytes, 0);
        push_f32(&mut bytes, 1.0);
        push_u16(&mut bytes, 1);
        push_f32(&mut bytes, 0.5);
        let block = parse_block("NiSkinData", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        let Block::SkinData(data) = block else {
            panic!("expected SkinData");
        };
        assert_eq!(data.unknown_byte, 0x01);
        assert_eq!(data.bones.len(), 1);
        assert_eq!(data.bones[0].transform.translation, [0.0, -1.55, 51.43]);
        assert_eq!(data.bones[0].weights, vec![(0, 1.0), (1, 0.5)]);
        // No trailing skin-partition ref in SE: ends exactly.
        assert_eq!(data.trailing_bytes, 0);
    }

    #[test]
    fn skin_partition_se_layout() {
        // One partition, 3 position-only vertices (16 bytes each:
        // vec3 + unused W), 1 triangle, 1 bone, 1 weight/vertex.
        let vdesc: u64 = 0x1 << 44;
        let mut bytes = Vec::new();
        push_u32(&mut bytes, 1); // NumPartitions
        push_u32(&mut bytes, 48); // VertexDataSize
        push_u32(&mut bytes, 16); // VertexSize
        push_u64(&mut bytes, vdesc);
        for pos in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            push_vec3(&mut bytes, pos);
            push_u32(&mut bytes, 0);
        }
        push_u16(&mut bytes, 3); // NumVertices
        push_u16(&mut bytes, 1); // NumTriangles
        push_u16(&mut bytes, 1); // NumBones
        push_u16(&mut bytes, 0); // NumStrips
        push_u16(&mut bytes, 1); // NumWeightsPerVertex
        push_u16(&mut bytes, 0); // Bones[0]
        bytes.push(1); // HasVertexMap
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, 1);
        push_u16(&mut bytes, 2);
        bytes.push(1); // HasVertexWeights
        push_f32(&mut bytes, 1.0);
        push_f32(&mut bytes, 1.0);
        push_f32(&mut bytes, 1.0);
        bytes.push(1); // HasFaces
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, 1);
        push_u16(&mut bytes, 2);
        bytes.push(1); // HasBoneIndices
        bytes.push(0);
        bytes.push(0);
        bytes.push(0);
        push_u16(&mut bytes, 0); // UnknownShort
        push_u64(&mut bytes, vdesc); // Duplicate vertex desc
        push_u16(&mut bytes, 0); // Duplicate triangles
        push_u16(&mut bytes, 1);
        push_u16(&mut bytes, 2);
        let block = parse_block("NiSkinPartition", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        let Block::SkinPartition(part) = block else {
            panic!("expected SkinPartition");
        };
        assert_eq!(part.vertices.len(), 3);
        assert_eq!(part.vertices[1].position, Some([1.0, 0.0, 0.0]));
        assert_eq!(part.partitions.len(), 1);
        let p = &part.partitions[0];
        assert_eq!(p.bones, vec![0]);
        assert_eq!(p.vertex_map, vec![0, 1, 2]);
        assert_eq!(p.triangles, vec![[0, 1, 2]]);
        assert_eq!(
            p.weights,
            vec![vec![(0, 1.0)], vec![(0, 1.0)], vec![(0, 1.0)]]
        );
        assert_eq!(part.trailing_bytes, 0);
    }

    #[test]
    fn bind_pose_weighted_transform() {
        // The mouth case from Delphine's FaceGen head: GeomSkin and
        // BoneSkin are identity, so the bind matrix is just the
        // head bone's world transform (0, -1.55, 120.34).
        let head_world = Transform {
            translation: [0.0, -1.55, 120.34],
            rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            scale: 1.0,
        };
        let m = head_world
            .compose(&Transform::identity())
            .compose(&Transform::identity());
        let v = apply_weighted(&[m], &[(0, 1.0)], [0.0, 6.0, 0.0]);
        assert!((v[0] - 0.0).abs() < 1e-4);
        assert!((v[1] - 4.45).abs() < 1e-4);
        assert!((v[2] - 120.34).abs() < 1e-4);

        // The head case: GeomSkin (0, 1.55, -120.34) is the exact
        // inverse of the head bone's world transform, so the bind
        // matrix is identity and the vertex is unchanged.
        let geom_skin = Transform {
            translation: [0.0, 1.55, -120.34],
            rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            scale: 1.0,
        };
        let m2 = head_world
            .compose(&Transform::identity())
            .compose(&geom_skin);
        let v2 = apply_weighted(&[m2], &[(0, 1.0)], [3.0, 4.0, 5.0]);
        assert!((v2[0] - 3.0).abs() < 1e-4);
        assert!((v2[1] - 4.0).abs() < 1e-4);
        assert!((v2[2] - 5.0).abs() < 1e-4);

        // Empty weights leave the vertex unchanged.
        assert_eq!(apply_weighted(&[m], &[], [1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]);

        // Two bones blend: half identity, half +10 in X.
        let shift = Transform {
            translation: [10.0, 0.0, 0.0],
            rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            scale: 1.0,
        };
        let v3 = apply_weighted(
            &[Transform::identity(), shift],
            &[(0, 0.5), (1, 0.5)],
            [0.0, 0.0, 0.0],
        );
        assert!((v3[0] - 5.0).abs() < 1e-4);

        // A 180-degree rotation about Y flips a normal's X and Z.
        let flip_y = Transform {
            translation: [0.0, 0.0, 0.0],
            rotation: [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]],
            scale: 1.0,
        };
        let n = rotate_weighted(&[flip_y], &[(0, 1.0)], [1.0, 0.0, 0.0]);
        assert!((n[0] + 1.0).abs() < 1e-4);
        assert!(n[1].abs() < 1e-4);
        assert!(n[2].abs() < 1e-4);
    }

    /// One quadratic float key: (time, value, fwd, bwd).
    fn push_quad_key(bytes: &mut Vec<u8>, time: f32, value: f32) {
        push_f32(bytes, time);
        push_f32(bytes, value);
        push_f32(bytes, 0.0);
        push_f32(bytes, 0.0);
    }

    /// The FarmhouseLDoor01 Open sequence (block 6), rebuilt from
    /// the `door_anim` probe's byte-exact dump: Z 0 -> -0.314159
    /// (-18 deg) over 0.6 s, X/Y flat, no translation/scale keys.
    fn door_open_block_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        push_u32(&mut bytes, 1); // num rotation keys (placeholder)
        push_u32(&mut bytes, 4); // rotation type: XYZ Euler
        // X group: 2 quadratic keys, flat at 0.
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, 2);
        push_quad_key(&mut bytes, 0.0, 0.0);
        push_quad_key(&mut bytes, 0.6, 0.0);
        // Y group: same.
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, 2);
        push_quad_key(&mut bytes, 0.0, 0.0);
        push_quad_key(&mut bytes, 0.6, 0.0);
        // Z group: 0 -> -pi/10 over 0.6 s.
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, 2);
        push_quad_key(&mut bytes, 0.0, 0.0);
        push_quad_key(&mut bytes, 0.6, -0.31415927);
        // No translation / scale keys.
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        bytes
    }

    #[test]
    fn transform_data_xyz_door_open() {
        let bytes = door_open_block_bytes();
        // 8 (header) + 3 * 40 (groups) + 8 (trans/scale) = 136,
        // the real block size in FarmhouseLDoor01.nif.
        assert_eq!(bytes.len(), 136);
        let block = parse_block("NiTransformData", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        let Block::TransformData(data) = block else {
            panic!("expected TransformData");
        };
        assert_eq!(data.x_keys.len(), 2);
        assert_eq!(data.y_keys.len(), 2);
        assert_eq!(data.z_keys.len(), 2);
        assert_eq!(data.z_keys[0].value, 0.0);
        assert!((data.z_keys[1].value - (-0.31415927)).abs() < 1e-6);
        assert_eq!(data.z_keys[1].time, 0.6);
        assert!(data.x_keys.iter().all(|k| k.value == 0.0));
    }

    #[test]
    fn transform_data_non_xyz_stays_unknown() {
        // Rotation type 0 (quaternion): not parsed, degrades to
        // Unknown instead of failing the file.
        let mut bytes = door_open_block_bytes();
        bytes[4..8].copy_from_slice(&0u32.to_le_bytes());
        let block = parse_block("NiTransformData", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        assert!(matches!(block, Block::Unknown { .. }));
    }

    #[test]
    fn transform_data_trailing_bytes_stay_unknown() {
        // One extra byte: the layout drifted, so the block
        // degrades to Unknown rather than returning wrong keys.
        let mut bytes = door_open_block_bytes();
        bytes.push(0);
        let block = parse_block("NiTransformData", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        assert!(matches!(block, Block::Unknown { .. }));
    }

    /// The secret door's string table (indices match the real
    /// RifRmSmSecretCabinetDoor01.nif: LeftDoor01 = 2,
    /// NiTransformController = 3, RightDoor01 = 4, Open = 8).
    fn secret_door_strings() -> Vec<String> {
        vec![
            "RifRmSmSecretCabinetDoor01".to_string(),
            "BSX".to_string(),
            "LeftDoor01".to_string(),
            "NiTransformController".to_string(),
            "RightDoor01".to_string(),
            "start".to_string(),
            "Sound: DRSWoodDelphineSecretOpen".to_string(),
            "end".to_string(),
            "Open".to_string(),
            "Sound: DRSWoodDelphineSecretClose".to_string(),
            "Close".to_string(),
        ]
    }

    /// The secret door's Open sequence block, byte for byte as
    /// the real file carries it: name "Open", 2 controlled
    /// links (interp 5 -> LeftDoor01, interp 7 -> RightDoor01),
    /// 34-byte tail. 12 + 2*29 + 34 = 104, the real block size.
    fn secret_door_open_sequence_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        push_u32(&mut bytes, 8); // name: "Open"
        push_u32(&mut bytes, 2); // num controlled
        push_u32(&mut bytes, 1); // unknown
        // Link 0: interp 5 -> LeftDoor01.
        push_i32(&mut bytes, 5);
        push_u32(&mut bytes, 3); // controller type
        bytes.push(0); // unknown byte
        push_u32(&mut bytes, 2); // target: "LeftDoor01"
        push_i32(&mut bytes, -1); // string palette: none
        push_u32(&mut bytes, 3); // controller type again
        push_i32(&mut bytes, -1);
        push_i32(&mut bytes, -1);
        // Link 1: interp 7 -> RightDoor01.
        push_i32(&mut bytes, 7);
        push_u32(&mut bytes, 3);
        bytes.push(0);
        push_u32(&mut bytes, 4); // target: "RightDoor01"
        push_i32(&mut bytes, -1);
        push_u32(&mut bytes, 3);
        push_i32(&mut bytes, -1);
        push_i32(&mut bytes, -1);
        // Tail: weight, text keys, cycle, freq, start, stop.
        push_f32(&mut bytes, 1.0);
        push_i32(&mut bytes, 9);
        push_u32(&mut bytes, 2);
        push_f32(&mut bytes, 1.0);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 1.0);
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, 0);
        bytes.push(0);
        bytes.push(0);
        bytes
    }

    #[test]
    fn controller_sequence_secret_door_links() {
        let bytes = secret_door_open_sequence_bytes();
        assert_eq!(bytes.len(), 104);
        let strings = secret_door_strings();
        let block = parse_block(
            "NiControllerSequence",
            &bytes,
            &strings,
            BS_VERSION_SKYRIM_SE,
        )
        .unwrap();
        let Block::ControllerSequence(seq) = block else {
            panic!("expected ControllerSequence");
        };
        assert_eq!(seq.name, "Open");
        assert_eq!(seq.controlled.len(), 2);
        assert_eq!(seq.controlled[0].interpolator, 5);
        assert_eq!(seq.controlled[0].target_name, "LeftDoor01");
        assert_eq!(seq.controlled[1].interpolator, 7);
        assert_eq!(seq.controlled[1].target_name, "RightDoor01");
    }

    #[test]
    fn controller_sequence_truncated_stays_unknown() {
        // Header + a partial first link: the layout drifted (or
        // the file is cut), so the block degrades to Unknown
        // rather than returning a wrong linkage.
        let mut bytes = secret_door_open_sequence_bytes();
        bytes.truncate(40);
        let strings = secret_door_strings();
        let block = parse_block(
            "NiControllerSequence",
            &bytes,
            &strings,
            BS_VERSION_SKYRIM_SE,
        )
        .unwrap();
        assert!(matches!(block, Block::Unknown { .. }));
    }

    #[test]
    fn transform_interpolator_data_ref() {
        // 36 bytes: 8 "unset" (-FLT_MAX) floats (translation 3
        // + quaternion 4 + scale 1), then the data ref at offset
        // 32 — as the secret door's interpolators carry it
        // ([5] -> data 6).
        let mut bytes = Vec::new();
        for _ in 0..8 {
            push_f32(&mut bytes, -f32::MAX);
        }
        push_i32(&mut bytes, 6);
        assert_eq!(bytes.len(), 36);
        let block =
            parse_block("NiTransformInterpolator", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        let Block::TransformInterpolator(interp) = block else {
            panic!("expected TransformInterpolator");
        };
        assert_eq!(interp.data, 6);
        // One extra byte: trailing bytes degrade to Unknown.
        bytes.push(0);
        let block =
            parse_block("NiTransformInterpolator", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        assert!(matches!(block, Block::Unknown { .. }));
    }

    #[test]
    fn transform_controller_parses() {
        // The sawmill's real SawGear controller (MillLogPile.nif
        // block [438], hex ffffffff4c000000803f0000000000000000
        // 7877c942b5010000b7010000): next -1, flags 0x004c,
        // frequency 1.0, phase 0.0, start 0.0, stop 100.7333 s,
        // target 437 (SawGear), interpolator 439.
        let mut bytes = Vec::new();
        push_i32(&mut bytes, -1);
        push_u16(&mut bytes, 0x004c);
        push_f32(&mut bytes, 1.0);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 100.7333);
        push_i32(&mut bytes, 437);
        push_i32(&mut bytes, 439);
        assert_eq!(bytes.len(), 30);
        let block =
            parse_block("NiTransformController", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        let Block::TransformController(ctl) = block else {
            panic!("expected TransformController");
        };
        assert_eq!(ctl.flags, 0x004c);
        assert_eq!(ctl.frequency, 1.0);
        assert_eq!(ctl.phase, 0.0);
        assert_eq!(ctl.start_time, 0.0);
        assert!((ctl.stop_time - 100.7333).abs() < 1e-4);
        assert_eq!(ctl.target, 437);
        assert_eq!(ctl.interpolator, 439);
        // One extra byte: trailing bytes degrade to Unknown.
        bytes.push(0);
        let block =
            parse_block("NiTransformController", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        assert!(matches!(block, Block::Unknown { .. }));
    }

    #[test]
    // 1.5708 is the REAL keyframe value from MillLogPile.nif's
    // SawGear data (Z constant), not a hand-typed approximation
    // of FRAC_PI_2 — replacing it with the constant would
    // falsify the test data.
    #[allow(clippy::approx_constant)]
    fn transform_data_tangent_order_and_key_types() {
        // A quadratic Y group whose second key carries tangents
        // 1.379 / 0.137 in file order — the blade's real 93.43 s
        // key (MillLogPile.nif block [440]). The file order is
        // BACKWARD first: 1.379 lands on bwd_tangent, 0.137 on
        // fwd_tangent. The group types are recorded too.
        let mut bytes = Vec::new();
        push_u32(&mut bytes, 1); // num rotation keys
        push_u32(&mut bytes, 4); // XYZ
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, 2);
        for t in [0.0f32, 100.7333] {
            push_f32(&mut bytes, t);
            push_f32(&mut bytes, 0.0);
            push_f32(&mut bytes, 0.0);
            push_f32(&mut bytes, 0.0);
        }
        // Y: 2 quadratic keys, tangents 1.379 / 0.137 on the
        // second key (file order: backward, forward).
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, 2);
        push_f32(&mut bytes, 64.5333);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 93.4333);
        push_f32(&mut bytes, 15.5724);
        push_f32(&mut bytes, 1.379);
        push_f32(&mut bytes, 0.137);
        // Z: 2 quadratic keys, constant 90 deg.
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, 2);
        for t in [0.0f32, 100.7333] {
            push_f32(&mut bytes, t);
            push_f32(&mut bytes, 1.5708);
            push_f32(&mut bytes, 0.0);
            push_f32(&mut bytes, 0.0);
        }
        // Translation: empty (no type field). Scale: empty.
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        let block = parse_block("NiTransformData", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        let Block::TransformData(data) = block else {
            panic!("expected TransformData");
        };
        assert_eq!(data.x_key_type, KeyInterpolation::Quadratic);
        assert_eq!(data.y_key_type, KeyInterpolation::Quadratic);
        assert_eq!(data.z_key_type, KeyInterpolation::Quadratic);
        assert_eq!(data.translation_key_type, KeyInterpolation::Constant);
        let key = &data.y_keys[1];
        assert!(
            (key.bwd_tangent - 1.379).abs() < 1e-6,
            "bwd {}",
            key.bwd_tangent
        );
        assert!(
            (key.fwd_tangent - 0.137).abs() < 1e-6,
            "fwd {}",
            key.fwd_tangent
        );
    }

    /// A synthetic sawmill NIF: node [0] `SawGear` carries a
    /// direct controller [1] -> interpolator [2] -> data [3]
    /// (Y 0 -> 1080 deg over the 15.37-50.67 s burst), node [4]
    /// `Broken` carries a controller [5] whose interpolator [6]
    /// points at a missing data block.
    ///
    /// 1.5708 is the REAL keyframe value from MillLogPile.nif's
    /// SawGear data (Z constant), not a hand-typed approximation
    /// of FRAC_PI_2.
    #[allow(clippy::approx_constant)]
    fn sawmill_nif() -> NifFile {
        let node = |name: &str, controller: i32| {
            Block::Node(NodeBlock {
                av: AvObject {
                    net: ObjectNet {
                        name: name.to_string(),
                        extra_data: Vec::new(),
                        controller,
                    },
                    flags: 0,
                    translation: [0.0; 3],
                    rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    scale: 1.0,
                    collision_object: -1,
                },
                children: Vec::new(),
                effects: Vec::new(),
            })
        };
        let key = |time: f32, value: f32| FloatKey {
            time,
            value,
            fwd_tangent: 0.0,
            bwd_tangent: 0.0,
        };
        let flat = vec![key(0.0, 0.0), key(100.7333, 0.0)];
        let data = TransformDataBlock {
            x_keys: flat.clone(),
            y_keys: vec![key(0.0, 0.0), key(15.3667, 0.0), key(50.6667, 18.8496)],
            z_keys: vec![key(0.0, 1.5708), key(100.7333, 1.5708)],
            x_key_type: KeyInterpolation::Quadratic,
            y_key_type: KeyInterpolation::Quadratic,
            z_key_type: KeyInterpolation::Quadratic,
            translation_keys: Vec::new(),
            translation_key_type: KeyInterpolation::Constant,
        };
        let blocks = vec![
            node("SawGear", 1),
            Block::TransformController(TransformControllerBlock {
                flags: 0x004c,
                frequency: 1.0,
                phase: 0.0,
                start_time: 0.0,
                stop_time: 100.7333,
                target: 0,
                interpolator: 2,
            }),
            Block::TransformInterpolator(TransformInterpolatorBlock { data: 3 }),
            Block::TransformData(data),
            node("Broken", 5),
            Block::TransformController(TransformControllerBlock {
                flags: 0x004c,
                frequency: 1.0,
                phase: 0.0,
                start_time: 0.0,
                stop_time: 10.0,
                target: 4,
                interpolator: 6,
            }),
            Block::TransformInterpolator(TransformInterpolatorBlock { data: 99 }),
        ];
        NifFile {
            version: 0x1402_0007,
            user_version: 12,
            bs_version: 100,
            header_string: String::new(),
            author: String::new(),
            process_script: String::new(),
            export_script: String::new(),
            strings: Vec::new(),
            blocks,
            root_indices: vec![0],
        }
    }

    #[test]
    fn direct_controllers_sawmill() {
        // SawGear resolves (controller + interpolator + data);
        // Broken's chain dangles and is skipped.
        let nif = sawmill_nif();
        let ctrls = nif.direct_controllers();
        assert_eq!(ctrls.len(), 1, "{ctrls:?}");
        let ctl = &ctrls[0];
        assert_eq!(ctl.node_name, "SawGear");
        assert_eq!(ctl.flags, 0x004c);
        assert_eq!(ctl.frequency, 1.0);
        assert_eq!(ctl.start_time, 0.0);
        assert!((ctl.stop_time - 100.7333).abs() < 1e-4);
        assert_eq!(ctl.data.y_keys.len(), 3);
        assert!((ctl.data.y_keys[2].value - 18.8496).abs() < 1e-6);
    }

    #[test]
    fn shader_emissive_resolves_color_times_multiple() {
        // Forge milestone: the coals' BSLightingShaderProperty
        // (white x2.39 in BlacksmithForgeMarker.nif) must reach
        // the renderer as linear emissive, not vanish. Missing /
        // black / zero-multiple properties resolve to black.
        let shader = |color: Color3, multiple: f32| {
            Block::LightingShaderProperty(LightingShaderPropertyBlock {
                net: ObjectNet {
                    name: String::new(),
                    extra_data: Vec::new(),
                    controller: -1,
                },
                shader_type: 0,
                shader_flags_1: 0,
                shader_flags_2: 0,
                uv_offset: [0.0, 0.0],
                uv_scale: [1.0, 1.0],
                texture_set: -1,
                emissive_color: color,
                emissive_multiple: multiple,
                texture_clamp_mode: 0,
                alpha: 1.0,
                refraction_strength: 0.0,
                glossiness: 0.0,
                specular_color: [0.0; 3],
                specular_strength: 0.0,
                lighting_effect_1: 0.0,
                lighting_effect_2: 0.0,
                extra: ShaderTypeExtra::None,
            })
        };
        let nif = NifFile {
            version: 0x1402_0007,
            user_version: 12,
            bs_version: 100,
            header_string: String::new(),
            author: String::new(),
            process_script: String::new(),
            export_script: String::new(),
            strings: Vec::new(),
            blocks: vec![
                shader([1.0, 1.0, 1.0], 2.39),
                shader([0.0, 0.0, 0.0], 5.0),
                shader([1.0, 0.0, 0.0], 0.0),
            ],
            root_indices: vec![0],
        };
        // White x2.39: linear(1.0) = 1.0, so exactly 2.39.
        assert_eq!(nif.shader_emissive(0), [2.39, 2.39, 2.39]);
        // Black color and zero multiple both resolve to black.
        assert_eq!(nif.shader_emissive(1), [0.0, 0.0, 0.0]);
        assert_eq!(nif.shader_emissive(2), [0.0, 0.0, 0.0]);
        // Missing index resolves to black.
        assert_eq!(nif.shader_emissive(99), [0.0, 0.0, 0.0]);
        assert_eq!(nif.shader_emissive(-1), [0.0, 0.0, 0.0]);
        // sRGB mid-grey converts (~0.214 linear), not 0.5.
        let mid = srgb_to_linear(0.5);
        assert!((mid - 0.2140).abs() < 0.001, "{mid}");
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert_eq!(srgb_to_linear(1.0), 1.0);
    }

    /// A synthetic secret-door NIF: the Open sequence links
    /// interp 5 -> LeftDoor01 and interp 7 -> RightDoor01, and
    /// the data blocks carry -101.2 deg and +101.2 deg over
    /// 1.0 s — the real values from
    /// RifRmSmSecretCabinetDoor01.nif.
    fn secret_door_nif() -> NifFile {
        let key = |time: f32, value: f32| FloatKey {
            time,
            value,
            fwd_tangent: 0.0,
            bwd_tangent: 0.0,
        };
        let flat = vec![key(0.0, 0.0), key(1.0, 0.0)];
        let left_data = TransformDataBlock {
            x_keys: flat.clone(),
            y_keys: flat.clone(),
            z_keys: vec![key(0.0, 0.0), key(1.0, -1.7671)],
            x_key_type: KeyInterpolation::Linear,
            y_key_type: KeyInterpolation::Linear,
            z_key_type: KeyInterpolation::Linear,
            translation_keys: Vec::new(),
            translation_key_type: KeyInterpolation::Constant,
        };
        let right_data = TransformDataBlock {
            x_keys: flat.clone(),
            y_keys: flat.clone(),
            z_keys: vec![key(0.0, 0.0), key(1.0, 1.7671)],
            x_key_type: KeyInterpolation::Linear,
            y_key_type: KeyInterpolation::Linear,
            z_key_type: KeyInterpolation::Linear,
            translation_keys: Vec::new(),
            translation_key_type: KeyInterpolation::Constant,
        };
        let mut blocks: Vec<Block> = (0..5)
            .map(|_| Block::Unknown {
                type_name: "NiNode".to_string(),
            })
            .collect();
        blocks.push(Block::TransformInterpolator(TransformInterpolatorBlock {
            data: 6,
        }));
        blocks.push(Block::TransformData(left_data));
        blocks.push(Block::TransformInterpolator(TransformInterpolatorBlock {
            data: 8,
        }));
        blocks.push(Block::TransformData(right_data));
        blocks.push(Block::ControllerSequence(ControllerSequenceBlock {
            name: "Open".to_string(),
            controlled: vec![
                ControlledBlock {
                    interpolator: 5,
                    target_name: "LeftDoor01".to_string(),
                },
                ControlledBlock {
                    interpolator: 7,
                    target_name: "RightDoor01".to_string(),
                },
            ],
        }));
        NifFile {
            version: 0x1402_0007,
            user_version: 12,
            bs_version: 100,
            header_string: String::new(),
            author: String::new(),
            process_script: String::new(),
            export_script: String::new(),
            strings: Vec::new(),
            blocks,
            root_indices: vec![0],
        }
    }

    #[test]
    fn open_angles_by_node_secret_door() {
        // The linkage is the answer: LeftDoor01 gets -101.2 deg,
        // RightDoor01 gets +101.2 deg — both panels swing toward
        // -Y (into the room), NOT toward +Y as the geometric
        // invariant would have said. This test pins the
        // controller-derived signs.
        let nif = secret_door_nif();
        let angles = nif.open_angles_by_node();
        assert_eq!(angles.len(), 2);
        let (left_angle, left_duration) = angles["LeftDoor01"];
        assert!((left_angle - (-1.7671)).abs() < 1e-6, "left {left_angle}");
        assert_eq!(left_duration, 1.0);
        let (right_angle, right_duration) = angles["RightDoor01"];
        assert!((right_angle - 1.7671).abs() < 1e-6, "right {right_angle}");
        assert_eq!(right_duration, 1.0);
    }

    #[test]
    fn open_angles_by_node_skips_unresolvable_links() {
        // A link whose interpolator ref points at nothing, and a
        // link whose data starts away from rest (Close-style),
        // are both skipped — the map holds only the good link.
        let mut nif = secret_door_nif();
        // Break the LeftDoor01 link: point it at a missing block.
        let Block::ControllerSequence(seq) = &mut nif.blocks[9] else {
            panic!("expected ControllerSequence at block 9");
        };
        seq.controlled[0].interpolator = 99;
        let angles = nif.open_angles_by_node();
        assert_eq!(angles.len(), 1);
        assert!(angles.contains_key("RightDoor01"));
        // No Open sequence at all: empty map.
        let Block::ControllerSequence(seq) = &mut nif.blocks[9] else {
            panic!("expected ControllerSequence at block 9");
        };
        seq.name = "Close".to_string();
        assert!(nif.open_angles_by_node().is_empty());
    }

    #[test]
    fn spinning_nodes_wheel() {
        // The Riverwood wheel's own keys (mill_probe): the Idle
        // sequence drives SawWaterWheel03's Y axis 0 -> -2π over
        // 16.6667 s, X/Z constant. One full turn per 16⅔ s at
        // -0.377 rad/s about local Y.
        let nif = mill_wheel_nif();
        let spins = nif.spinning_nodes();
        assert_eq!(spins.len(), 1, "{spins:?}");
        let spin = &spins[0];
        assert_eq!(spin.node_name, "SawWaterWheel03");
        assert_eq!(spin.axis, 1);
        assert_eq!(spin.axis_unit(), [0.0, 1.0, 0.0]);
        assert!(
            (spin.angular_velocity - (-0.37699112)).abs() < 1e-4,
            "omega {}",
            spin.angular_velocity
        );
        assert!(
            (spin.period - 16.6667).abs() < 1e-3,
            "period {}",
            spin.period
        );
    }

    #[test]
    fn spinning_nodes_ignores_doors() {
        // Door swings never span 2π: the secret door's Open
        // sequence (±101°) yields no spinning node.
        let nif = secret_door_nif();
        assert!(nif.spinning_nodes().is_empty());
    }

    #[test]
    fn spinning_nodes_skips_unresolvable_links() {
        // A link whose interpolator ref points at nothing, and
        // a link whose second axis also moves, are both skipped.
        let mut nif = mill_wheel_nif();
        let Block::ControllerSequence(seq) = &mut nif.blocks[4] else {
            panic!("expected ControllerSequence at block 4");
        };
        seq.controlled[0].interpolator = 99;
        assert!(nif.spinning_nodes().is_empty());
        let mut nif = mill_wheel_nif();
        let Block::TransformData(data) = &mut nif.blocks[6] else {
            panic!("expected TransformData at block 6");
        };
        // Z now moves too: no longer a single-axis spin.
        data.z_keys.push(FloatKey {
            time: 16.6667,
            value: 0.5,
            fwd_tangent: 0.0,
            bwd_tangent: 0.0,
        });
        assert!(nif.spinning_nodes().is_empty());
    }

    // -6.2832 / 1.5708 are the REAL keyframe values from the
    // wheel NIF (Y 0 -> -2π, Z constant 90°), not hand-typed
    // approximations of TAU / FRAC_PI_2.
    #[allow(clippy::approx_constant)]
    fn mill_wheel_nif() -> NifFile {
        // Blocks mirror the real wheel NIF's layout ([4] the
        // Idle sequence, [5] the wheel interpolator, [6] its
        // data) with the probe-measured keys.
        let key = |time: f32, value: f32| FloatKey {
            time,
            value,
            fwd_tangent: 0.0,
            bwd_tangent: 0.0,
        };
        let data = TransformDataBlock {
            x_keys: vec![key(0.0, 0.0)],
            y_keys: vec![key(0.0, 0.0), key(16.6667, -6.2832)],
            z_keys: vec![key(0.0, 1.5708)],
            x_key_type: KeyInterpolation::Linear,
            y_key_type: KeyInterpolation::Linear,
            z_key_type: KeyInterpolation::Linear,
            translation_keys: Vec::new(),
            translation_key_type: KeyInterpolation::Constant,
        };
        let mut blocks: Vec<Block> = (0..5)
            .map(|_| Block::Unknown {
                type_name: "NiNode".to_string(),
            })
            .collect();
        blocks[4] = Block::ControllerSequence(ControllerSequenceBlock {
            name: "Idle".to_string(),
            controlled: vec![ControlledBlock {
                interpolator: 5,
                target_name: "SawWaterWheel03".to_string(),
            }],
        });
        blocks.push(Block::TransformInterpolator(TransformInterpolatorBlock {
            data: 6,
        }));
        blocks.push(Block::TransformData(data));
        NifFile {
            version: 0x1402_0007,
            user_version: 12,
            bs_version: 100,
            header_string: String::new(),
            author: String::new(),
            process_script: String::new(),
            export_script: String::new(),
            strings: Vec::new(),
            blocks,
            root_indices: vec![0],
        }
    }

    #[test]
    fn alpha_blend_mode_decode() {
        // 0x10ed: blend on, src SRC_ALPHA, dst INV_SRC_ALPHA.
        assert_eq!(alpha_blend_mode(0x10ed), AlphaBlend::Blend);
        // 0x100d: blend on, src SRC_ALPHA, dst ONE = additive.
        assert_eq!(alpha_blend_mode(0x100d), AlphaBlend::Additive);
        // Foliage: test-only, bit 0 clear — no blending.
        assert_eq!(alpha_blend_mode(0x12ec), AlphaBlend::Opaque);
        assert_eq!(alpha_blend_mode(0x1200), AlphaBlend::Opaque);
        assert_eq!(alpha_blend_mode(0x0000), AlphaBlend::Opaque);
        // Bit 0 set with dst ONE and the test bit on: additive
        // (dst decides; the renderer lets the cutoff win).
        assert_eq!(alpha_blend_mode(0x0201), AlphaBlend::Additive);
    }

    fn push_sized_string(bytes: &mut Vec<u8>, s: &str) {
        push_u32(bytes, s.len() as u32);
        bytes.extend_from_slice(s.as_bytes());
    }

    /// An effect-shader block blob in the real fog layout: net,
    /// flags, UVs, source texture, clamp, 4 floats, emissive
    /// Color4, 2 floats, then the optional gradient string (or a
    /// trailing f32 0.0 when `gradient` is `None` — the block
    /// [22] shape).
    fn effect_shader_blob(gradient: Option<&str>) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_u32(&mut bytes, 0xFFFF_FFFF); // name: none
        push_u32(&mut bytes, 0); // num extra data
        push_i32(&mut bytes, -1); // controller
        push_u32(&mut bytes, 0xe000_0078); // shader flags 1
        push_u32(&mut bytes, 0x4000_0030); // shader flags 2
        push_f32(&mut bytes, 0.0); // UV offset
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 1.0); // UV scale
        push_f32(&mut bytes, 1.0);
        push_sized_string(&mut bytes, "textures\\effects\\FXFogHeavy.dds");
        push_u32(&mut bytes, 0x0000_a503); // clamp mode
        for v in [0.0, 0.0, 1.0, 0.0] {
            push_f32(&mut bytes, v);
        }
        for v in [0.775, 0.775, 0.775, 1.0] {
            push_f32(&mut bytes, v);
        }
        push_f32(&mut bytes, 0.75);
        push_f32(&mut bytes, 50.0);
        match gradient {
            Some(g) => push_sized_string(&mut bytes, g),
            None => push_f32(&mut bytes, 0.0),
        }
        bytes
    }

    #[test]
    fn effect_shader_property_parses() {
        let strings: Vec<String> = Vec::new();
        let blob = effect_shader_blob(Some("textures\\effects\\gradients\\GradCobWeb.dds"));
        // The real block [4] is 160 bytes; the blob must match.
        assert_eq!(blob.len(), 160);
        let mut reader = Reader::new(&blob);
        let shader = parse_effect_shader_property(&mut reader, &strings).unwrap();
        assert_eq!(shader.shader_flags_1, 0xe000_0078);
        assert_eq!(shader.shader_flags_2, 0x4000_0030);
        assert_eq!(shader.source_texture, "textures\\effects\\FXFogHeavy.dds");
        assert_eq!(shader.texture_clamp_mode, 0x0000_a503);
        assert_eq!(shader.emissive, [0.775, 0.775, 0.775, 1.0]);
        assert_eq!(shader.unknown_post_emissive, [0.75, 50.0]);
        assert_eq!(
            shader.gradient_texture.as_deref(),
            Some("textures\\effects\\gradients\\GradCobWeb.dds")
        );
        assert_eq!(reader.remaining(), 0);

        // No gradient string: the trailing f32 0.0 parses as an
        // empty string and the gradient is None (block [22]'s
        // shape; 119 here vs 125 there because its source texture
        // name is 6 chars longer).
        let blob = effect_shader_blob(None);
        assert_eq!(blob.len(), 119);
        let mut reader = Reader::new(&blob);
        let shader = parse_effect_shader_property(&mut reader, &strings).unwrap();
        assert_eq!(shader.gradient_texture, None);
        assert_eq!(reader.remaining(), 0);
    }

    // ---- collision (bhk) parsing ----

    #[test]
    fn quat_to_mat3_90deg_z() {
        // 90 degrees about Z: (1,0,0) -> (0,1,0).
        let q = [
            0.0,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
        ];
        let m = quat_to_mat3(q);
        let eps = 1e-5;
        assert!((m[0][0] - 0.0).abs() < eps);
        assert!((m[0][1] - -1.0).abs() < eps);
        assert!((m[1][0] - 1.0).abs() < eps);
        assert!((m[1][1] - 0.0).abs() < eps);
        assert!((m[2][2] - 1.0).abs() < eps);
        // Zero-length quaternion degrades to identity, never NaN.
        let m0 = quat_to_mat3([0.0, 0.0, 0.0, 0.0]);
        assert_eq!(m0, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    }

    #[test]
    fn mat4_compose_and_point() {
        let t = Transform {
            translation: [1.0, 2.0, 3.0],
            rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            scale: 2.0,
        };
        let m = mat4_from_transform(&t);
        // Scale 2, then translate: (1,0,0) -> (3,2,3).
        assert_eq!(mat4_point(&m, [1.0, 0.0, 0.0]), [3.0, 2.0, 3.0]);
        // mat4_mul(a, b) applies b first.
        let a = mat4_from_transform(&Transform {
            translation: [10.0, 0.0, 0.0],
            rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            scale: 1.0,
        });
        let b = mat4_from_transform(&Transform {
            translation: [0.0, 20.0, 0.0],
            rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            scale: 1.0,
        });
        let ab = mat4_mul(&a, &b);
        assert_eq!(mat4_point(&ab, [0.0, 0.0, 0.0]), [10.0, 20.0, 0.0]);
        assert_eq!(
            mat4_point(&mat4_scale([2.0, 2.0, 2.0]), [1.0, 2.0, 3.0]),
            [2.0, 4.0, 6.0]
        );
    }

    fn rigid_body_blob(response: u8, no_collision_flag: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_u32(&mut bytes, 4); // shape ref
        bytes.push(1); // filter layer STATIC
        bytes.push(if no_collision_flag { 0x40 } else { 0 });
        push_u16(&mut bytes, 0); // group
        bytes.extend_from_slice(&[0u8; 20]);
        bytes.push(response);
        bytes.extend_from_slice(&[0u8; 3]);
        push_f32(&mut bytes, 0.0); // mass
        bytes.push(1); // second filter layer
        bytes.push(0);
        push_u16(&mut bytes, 0);
        push_f32(&mut bytes, 0.5); // friction
        push_f32(&mut bytes, 0.4); // restitution
        bytes.push(response);
        bytes.extend_from_slice(&[0u8; 7]);
        bytes.push(5); // motion system
        bytes.push(0); // quality
        bytes.extend_from_slice(&[0u8; 170]);
        assert_eq!(bytes.len(), 228);
        bytes
    }

    #[test]
    fn collision_object_and_rigid_body_parse() {
        // The exact WalkwayStairs1 bytes: target 0, flags 0x81,
        // body block 5.
        let block = parse_block(
            "bhkCollisionObject",
            &[0, 0, 0, 0, 0x81, 0, 5, 0, 0, 0],
            &[],
            BS_VERSION_SKYRIM_SE,
        )
        .unwrap();
        let Block::CollisionObject(obj) = block else {
            panic!("expected CollisionObject");
        };
        assert_eq!(obj.target, 0);
        assert_eq!(obj.body, 5);

        let block = parse_block(
            "bhkRigidBody",
            &rigid_body_blob(1, false),
            &[],
            BS_VERSION_SKYRIM_SE,
        )
        .unwrap();
        let Block::RigidBody(body) = block else {
            panic!("expected RigidBody");
        };
        assert_eq!(body.shape, 4);
        assert_eq!(body.layer, 1);
        assert!(body.collides);
        assert!(body.transform.is_none());

        // Response 2 (or 3) = non-colliding.
        let block = parse_block(
            "bhkRigidBody",
            &rigid_body_blob(2, false),
            &[],
            BS_VERSION_SKYRIM_SE,
        )
        .unwrap();
        let Block::RigidBody(body) = block else {
            panic!("expected RigidBody");
        };
        assert!(!body.collides);
        // Filter flag 0x40 = no collision.
        let block = parse_block(
            "bhkRigidBody",
            &rigid_body_blob(1, true),
            &[],
            BS_VERSION_SKYRIM_SE,
        )
        .unwrap();
        let Block::RigidBody(body) = block else {
            panic!("expected RigidBody");
        };
        assert!(!body.collides);
    }

    #[test]
    fn rigid_body_t_transform_and_bad_quat() {
        // The T transform sits INSIDE the body at @52 (right after
        // response2 @48 + 3 cinfo-tail bytes) — Bridge01's real
        // bhkRigidBodyT is 250 bytes, the same as a non-T body,
        // with translation (0,0,-1.635) m @52 and identity quat
        // @68. Build the blob in that shape.
        let mut bytes = Vec::new();
        push_u32(&mut bytes, 4); // shape ref
        bytes.push(1); // filter layer STATIC
        bytes.push(0);
        push_u16(&mut bytes, 0); // group
        bytes.extend_from_slice(&[0u8; 20]);
        bytes.push(1); // response
        bytes.extend_from_slice(&[0u8; 3]);
        push_f32(&mut bytes, 0.0); // mass
        bytes.push(1); // second filter layer
        bytes.push(0);
        push_u16(&mut bytes, 0);
        push_f32(&mut bytes, 0.5); // friction
        push_f32(&mut bytes, 0.4); // restitution
        bytes.push(1); // response2 @48
        bytes.extend_from_slice(&[0u8; 3]); // cinfo tail @49..52
        assert_eq!(bytes.len(), 52);
        push_vec3(&mut bytes, [1.0, 2.0, 3.0]); // translation @52
        push_f32(&mut bytes, 0.0); // w @64
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 1.0); // identity quat @68
        assert_eq!(bytes.len(), 84);
        bytes.extend_from_slice(&[0u8; 166]); // trailing data
        assert_eq!(bytes.len(), 250);
        let block = parse_block("bhkRigidBodyT", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        let Block::RigidBody(body) = block else {
            panic!("expected RigidBody");
        };
        // Translation is metres -> Skyrim units (x70).
        let t = body.transform.expect("T body carries a transform");
        assert_eq!(t.translation, [70.0, 140.0, 210.0]);
        assert_eq!(
            t.rotation,
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        );
        assert_eq!(body.transform.as_ref().unwrap().scale, 1.0);

        // Bad quat (zero length) degrades the whole block to
        // Unknown — collision is never load-bearing.
        let mut bad = bytes.clone();
        // Zero the quat @68..84.
        for b in bad.iter_mut().skip(68).take(16) {
            *b = 0;
        }
        let block = parse_block("bhkRigidBodyT", &bad, &[], BS_VERSION_SKYRIM_SE).unwrap();
        assert!(matches!(block, Block::Unknown { .. }));
    }

    #[test]
    fn capsule_and_box_shape_parse() {
        let mut bytes = vec![0u8; 16];
        push_vec3(&mut bytes, [0.0, 0.0, 0.0]); // A
        push_f32(&mut bytes, 1.0); // radius A
        push_vec3(&mut bytes, [0.0, 0.0, 2.0]); // B
        push_f32(&mut bytes, 1.5); // radius B (wins: max)
        let block = parse_block("bhkCapsuleShape", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        let Block::CapsuleShape(cap) = block else {
            panic!("expected CapsuleShape");
        };
        assert_eq!(cap.a, [0.0, 0.0, 0.0]);
        assert_eq!(cap.b, [0.0, 0.0, 2.0]);
        assert_eq!(cap.radius, 1.5);

        let mut bytes = vec![0u8; 16];
        push_vec3(&mut bytes, [1.0, 2.0, 3.0]); // half-extents
        let block = parse_block("bhkBoxShape", &bytes, &[], BS_VERSION_SKYRIM_SE).unwrap();
        let Block::BoxShape(b) = block else {
            panic!("expected BoxShape");
        };
        assert_eq!(b.half_extents, [1.0, 2.0, 3.0]);
    }

    /// A minimal valid `bhkCompressedMeshShapeData` blob: one
    /// chunk, `vertices` quantized points, `indices` indices,
    /// `strips` strip lengths. Quantization error 0.001, so a
    /// quantized unit is 70 * 0.001 = 0.07 Skyrim units.
    fn compressed_mesh_blob(
        material_layer: u8,
        vertices: &[[u16; 3]],
        indices: &[u16],
        strips: &[u16],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_u32(&mut bytes, 17); // bits
        push_u32(&mut bytes, 18); // winding bits
        bytes.extend_from_slice(&[0u8; 8]); // masks
        push_f32(&mut bytes, 0.001); // quantization error
        bytes.extend_from_slice(&[0u8; 32]); // AABB
        bytes.extend_from_slice(&[0u8; 2]); // modes
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 1); // materials
        push_u32(&mut bytes, 0x1234); // code
        bytes.push(material_layer);
        bytes.extend_from_slice(&[0u8; 3]);
        push_u32(&mut bytes, 0); // named materials
        push_u32(&mut bytes, 1); // transforms
        push_vec3(&mut bytes, [0.0, 0.0, 0.0]);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 1.0);
        push_u32(&mut bytes, 0); // big vertices
        push_u32(&mut bytes, 0); // big triangles
        push_u32(&mut bytes, 1); // chunks
        push_vec3(&mut bytes, [0.0, 0.0, 0.0]); // chunk translation
        push_f32(&mut bytes, 0.0);
        push_u32(&mut bytes, 0); // material
        push_u16(&mut bytes, 0xFFFF); // reference
        push_u16(&mut bytes, 0); // transform index
        push_u32(&mut bytes, (vertices.len() * 3) as u32);
        for v in vertices {
            for c in v {
                push_u16(&mut bytes, *c);
            }
        }
        push_u32(&mut bytes, indices.len() as u32);
        for i in indices {
            push_u16(&mut bytes, *i);
        }
        push_u32(&mut bytes, strips.len() as u32);
        for s in strips {
            push_u16(&mut bytes, *s);
        }
        push_u32(&mut bytes, 0); // weld
        push_u32(&mut bytes, 0); // convex pieces
        bytes
    }

    #[test]
    fn compressed_mesh_decode_minimal() {
        // One flat triangle: (0,0,0), (70,0,0), (0,70,0).
        let blob =
            compressed_mesh_blob(1, &[[0, 0, 0], [1000, 0, 0], [0, 1000, 0]], &[0, 1, 2], &[]);
        let decoded = decode_compressed_mesh(&blob).expect("decodes");
        assert_eq!(
            decoded.vertices,
            [[0.0, 0.0, 0.0], [70.0, 0.0, 0.0], [0.0, 70.0, 0.0]]
        );
        assert_eq!(decoded.groups.len(), 1);
        assert_eq!(decoded.groups[0].0, 0x1234);
        assert_eq!(decoded.groups[0].1, 1);
        assert_eq!(decoded.groups[0].2, vec![[0, 1, 2]]);

        // Strip of 4 vertices -> 2 triangles, alternating winding.
        let blob = compressed_mesh_blob(
            1,
            &[[0, 0, 0], [1000, 0, 0], [1000, 1000, 0], [0, 1000, 0]],
            &[0, 1, 2, 3],
            &[4],
        );
        let decoded = decode_compressed_mesh(&blob).expect("decodes");
        assert_eq!(decoded.groups[0].2, vec![[0, 1, 2], [2, 1, 3]]);
    }

    #[test]
    fn compressed_mesh_decode_rejects() {
        let good =
            compressed_mesh_blob(1, &[[0, 0, 0], [1000, 0, 0], [0, 1000, 0]], &[0, 1, 2], &[]);
        // Wrong index format.
        let mut bad = good.clone();
        bad[0] = 16;
        assert!(decode_compressed_mesh(&bad).is_none());
        // Trailing byte.
        let mut bad = good.clone();
        bad.push(0);
        assert!(decode_compressed_mesh(&bad).is_none());
        // Referenced chunk (reference != 0xFFFF).
        let mut bad = good.clone();
        // Find the 0xFFFF reference: it sits right after the chunk
        // material u32; locate by scanning for the pattern.
        let pos = bad
            .windows(2)
            .position(|w| w == [0xFF, 0xFF])
            .expect("reference marker");
        bad[pos] = 0;
        assert!(decode_compressed_mesh(&bad).is_none());
        // NONCOLLIDABLE material -> no physical triangles.
        let blob = compressed_mesh_blob(
            15,
            &[[0, 0, 0], [1000, 0, 0], [0, 1000, 0]],
            &[0, 1, 2],
            &[],
        );
        assert!(decode_compressed_mesh(&blob).is_none());
        // Truncated blob.
        assert!(decode_compressed_mesh(&good[..good.len() - 4]).is_none());
    }
}
