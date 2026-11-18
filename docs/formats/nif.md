# NIF — NetImmerse File Format

Source: the NifTools format specification, `nif.xml`
(`https://raw.githubusercontent.com/niftools/nifxml/master/nif.xml`),
the machine-readable format spec maintained by the NifTools community.
Notes below are our own summary of that public spec.

## What it is

The 3D model format — meshes, materials, skeletons, animations, and
scene graphs, from the Gamebryo/NetImmerse lineage. Skyrim SE uses NIF
version 20.2.0.7. All numbers are little-endian.

## Versions

Three version numbers matter, and they are not the same thing:

| Number | Skyrim LE | Skyrim SE | Where it lives |
| --- | --- | --- | --- |
| NIF version | 20.2.0.7 (`0x14020007`) | 20.2.0.7 (`0x14020007`) | Header, after the header string |
| User version | 12 | 12 | Header |
| BS stream version | 83 | 100 | BSStreamHeader, inside the header |

The **BS stream version** drives most layout choices (flag widths,
which fields exist), not the user version. Fallout 4 is BS version
130 with the same NIF version; this reader does not support it yet.

The header string is the line
`"Gamebryo File Format, Version 20.2.0.7\n"` (ends with a newline).

## Layout

```
Header (string table, block type table, block sizes)
Blocks (one object per block, in file order)
Footer (root block indices)
```

A NIF is a flat list of typed blocks. Blocks reference each other by
index (`Ref`, an i32; −1 is null). The scene graph is a tree of
`NiNode` blocks whose children are other nodes or shapes; the footer
lists the root blocks (usually block 0).

## Header

| Field | Size | Notes |
| --- | --- | --- |
| Header String | line | `"Gamebryo File Format, Version 20.2.0.7\n"` |
| Version | u32 | `0x14020007` for Skyrim |
| Endian Type | byte | 1 = little-endian (Skyrim is always little-endian) |
| User Version | u32 | 12 for Skyrim |
| Num Blocks | u32 | Number of objects |
| BS Header | struct | BSStreamHeader (see below) |
| Num Block Types | u16 | Number of distinct block type names |
| Block Types | SizedString[] | The type names, e.g. `"NiNode"`, `"BSTriShape"` |
| Block Type Index | u16[] | One per block: index into Block Types |
| Block Size | u32[] | One per block: byte size of that block |
| Num Strings | u32 | Entries in the string table |
| Max String Length | u32 | Longest string in the table |
| Strings | SizedString[] | The string table |
| Num Groups | u32 | Usually 0 |
| Groups | u32[] | Group indices (unused by the reader) |

BSStreamHeader (present for Skyrim):

| Field | Size | Notes |
| --- | --- | --- |
| BS Version | u32 | 83 = Skyrim LE, 100 = Skyrim SE |
| Author | ExportString | Exporter author string |
| Process Script | ExportString | Only when BS Version < 131 |
| Export Script | ExportString | |
| Max Filepath | ExportString | Only when BS Version ≥ 103 (absent for Skyrim) |

The per-block sizes (present since NIF 20.2.0.5) are what make the
format safely skippable: a reader can walk blocks it does not
understand by jumping over them.

## Basic types

| Type | Layout |
| --- | --- |
| `SizedString` | u32 length + bytes, no NUL terminator |
| `string` | Since 20.1.0.3: u32 index into the header string table (`0xFFFFFFFF` = empty). Object names and material names use this |
| `FilePath` | Since 20.1.0.3: same as `string` (u32 string-table index) |
| `ExportString` | byte length (includes the NUL terminator) + bytes |
| `Ref` | i32 block index; −1 = null |
| `bool` | 1 byte (0 = false, anything else = true) |
| `hfloat` | 16-bit IEEE half float (2 bytes) |
| `normbyte` | 1 signed byte, a float in [−1, 1] as value / 127, clamped |
| `Vector3` / `TexCoord` / `Color3` / `Color4` | 3 / 2 / 3 / 4 floats |
| `Matrix33` | 9 floats, row-major as stored |
| `Matrix22` | 4 floats |
| `Triangle` | 3 × u16 vertex indices |
| `NiBound` | Center Vector3 + Radius float (a bounding sphere) |

## Blocks the reader parses

| Block | What it holds |
| --- | --- |
| `NiNode` | Scene-graph node: children + effects |
| `NiTriShape` / `NiTriStrips` | Shape: references to geometry data + shader |
| `BSLODTriShape` | NiTriShape variant with three LOD sizes |
| `BSTriShape` | SE-native shape: vertex data inline in the block |
| `BSDynamicTriShape` | BSTriShape + dynamic positions (FaceGen heads); triangles live in the skin partition |
| `NiTriShapeData` / `NiTriStripsData` | The actual mesh data for the Ni* shapes |
| `NiTexturingProperty` | Legacy Gamebryo texture slots (parsed, see below) |
| `NiSourceTexture` | A texture file reference |
| `BSLightingShaderProperty` | The Skyrim shader: flags, colors, texture set |
| `BSShaderTextureSet` | The texture paths for a shader (diffuse, normal, ...) |
| `NiAlphaProperty` | Transparency flags + threshold |
| `NiMaterialProperty` | Legacy material colors |

Any other block type (skinning, animation, collision, particles,
lights, ...) is skipped by its header block size.

### NiObjectNET — the common base

Every named object starts with this (in file order):

| Field | Size | Notes |
| --- | --- | --- |
| Shader Type | u32 | **Only** for `BSLightingShaderProperty`, and it comes *first*, before the name |
| Name | string | u32 string-table index |
| Num Extra Data List | u32 | |
| Extra Data List | Ref[] | Extra data objects |
| Controller | Ref | Time controller (animation), usually −1 |

### NiAVObject — the scene-graph base

NiObjectNET, then:

| Field | Size | Notes |
| --- | --- | --- |
| Flags | u32 | BS version > 26 (Skyrim); a u16 in older streams |
| Translation | Vector3 | Local position |
| Rotation | Matrix33 | Local rotation |
| Scale | float | Uniform scale |
| Collision Object | Ref | Collision object, usually −1 |

Note: the `Num Properties` / `Properties` list exists only for BS
version ≤ 34 (Oblivion and earlier). Skyrim shapes attach their
shader and alpha property through `NiGeometry` refs instead (below).

### NiNode

NiAVObject, then Num Children (u32), Children (Ref[]), Num Effects
(u32) + Effects (Ref[], BS version < 130).

### NiGeometry (NiTriShape / NiTriStrips / BSLODTriShape)

NiAVObject, then:

| Field | Size | Notes |
| --- | --- | --- |
| Data | Ref | `NiTriShapeData` / `NiTriStripsData` |
| Skin Instance | Ref | `NiSkinInstance` (bind-pose skinning, M12; LE-style skinning does not occur in SE data) |
| Material Data | struct | Num Materials u32; Material Name (string) × N; Material Extra Data i32 × N; Active Material i32; Material Needs Update bool |
| Shader Property | Ref | `BSLightingShaderProperty` |
| Alpha Property | Ref | `NiAlphaProperty` |

`BSLODTriShape` adds three trailing u32s (LOD0/1/2 sizes) after the
NiGeometry fields. `NiTriShape` and `NiTriStrips` add no fields of
their own. `BSSegmentedTriShape` is not parsed (skipped by size).

### NiGeometryData — mesh data

| Field | Size | Notes |
| --- | --- | --- |
| Group ID | i32 | Always 0 |
| Num Vertices | u16 | |
| Keep Flags | byte | |
| Compress Flags | byte | |
| Has Vertices | bool | |
| Vertices | Vector3[] | Num Vertices entries, if present |
| BS Data Flags | u16 | `BSGeometryDataFlags` (see below) |
| Material CRC | u32 | |
| Has Normals | bool | |
| Normals | Vector3[] | If present |
| Tangents | Vector3[] | If Has Normals *and* BS Data Flags bit 12 set |
| Bitangents | Vector3[] | Same condition |
| Bounding Sphere | NiBound | |
| Has Vertex Colors | bool | |
| Vertex Colors | Color4[] | If present |
| UV Sets | TexCoord[] | Count = BS Data Flags bit 0 (0 or 1 set for Skyrim), stored set-major: each set is Num Vertices coords |
| Consistency Flags | u16 | `CT_MUTABLE 0x0000`, `CT_STATIC 0x4000`, `CT_VOLATILE 0x8000` |
| Additional Data | Ref | Usually −1 |

`BSGeometryDataFlags` (u16): bit 0 = Has UV; bits 6–11 = Havok
material; bit 12 (`0x1000`) = Has Tangents.

### NiTriBasedGeomData

Adds Num Triangles (u16) after NiGeometryData.

### NiTriShapeData

Num Triangle Points (u32 = Num Triangles × 3), Has Triangles (bool),
Triangles (`Triangle` × Num Triangles, if present), Num Match Groups
(u16), Match Groups (each: Num Vertices u16 + vertex indices u16[]).
Match groups record shared-normal vertex groups; the reader parses
them but the mesh assembler does not need them.

### NiTriStripsData

Num Strips (u16), Strip Lengths (u16[]), Has Points (bool), Points
(u16, Num Strips × Strip Lengths, if present). Strips triangulate
with alternating winding: triangle *i* of a strip is
(v[i], v[i+1], v[i+2]) for even *i* and (v[i+1], v[i], v[i+2]) for odd
*i*; degenerate triangles (any repeated index) are dropped.

### BSTriShape — the SE-native shape

Most Skyrim SE meshes use this, not NiTriShape. It inherits
NiAVObject directly (not NiGeometry), so its vertex data is inline
and there is no separate data block and no Material Data:

| Field | Size | Notes |
| --- | --- | --- |
| Bounding Sphere | NiBound | |
| Skin | Ref | Skin instance (bind-pose reconstruction, M12) |
| Shader Property | Ref | `BSLightingShaderProperty` |
| Alpha Property | Ref | `NiAlphaProperty` |
| Vertex Desc | u64 | `BSVertexDesc` (see below) |
| Num Triangles | u16 | (u32 for Fallout 4) |
| Num Vertices | u16 | |
| Data Size | u32 | Stored size of the vertex data + triangles |
| Vertex Data | struct[] | Num Vertices × `BSVertexDataSSE`, if Data Size > 0 |
| Triangles | Triangle[] | Num Triangles entries, if Data Size > 0 |
| Particle Data Size | u32 | Stored size of the particle copy (below) |
| Particle Vertices | HalfVector3[] | Half-precision copy, if Particle Data Size > 0 |
| Particle Normals | HalfVector3[] | Same |
| Particle Triangles | Triangle[] | Same |

The particle copy is a half-precision duplicate of the mesh the game
uses for its particle system; the reader parses it but the mesh
assembler ignores it.

`BSVertexDesc` (u64): bits 0–3 = vertex size in 4-byte units, bits
4–7 = dynamic vertex size in 4-byte units (the `BSDynamicTriShape`
record stride — 16 bytes in every real file swept), bits 8–43 =
per-attribute offsets, bits 44–55 = `VertexAttribute` flags:

| Bit | Attribute |
| --- | --- |
| 0 | Vertex (position) |
| 1 | UVs |
| 2 | UVs_2 (second UV set; no field in the SE vertex struct) |
| 3 | Normals |
| 4 | Tangents |
| 5 | Vertex Colors |
| 6 | Skinned (bone weights + indices) |
| 7 | Land Data |
| 8 | Eye Data |
| 9 | Instance |
| 10 | Full Precision |

`BSVertexDataSSE`, one per vertex, fields present by the attribute
bits (arg = Vertex Desc >> 44):

| Condition | Field | Notes |
| --- | --- | --- |
| bit 0 | Vertex (Vector3) | Full-precision float position |
| bits 0+4 | Bitangent X (float) | Else an Unused W (u32) when bit 0 is set without bit 4 |
| bit 1 | UV (HalfTexCoord) | Half floats |
| bit 3 | Normal (ByteVector3) + Bitangent Y (normbyte) | Byte normals, / 127 |
| bits 3+4 | Tangent (ByteVector3) + Bitangent Z (normbyte) | |
| bit 5 | Vertex Colors (ByteColor4) | 4 bytes, 0–255 |
| bit 6 | Bone Weights (4 × hfloat) + Bone Indices (4 × byte) | Skinned; the partition's weights are authoritative, these are read to walk the stream |
| bit 8 | Eye Data (float) | |

### BSDynamicTriShape — the FaceGen shape

A `BSTriShape` with a dynamic section appended after the particle
data. Used for meshes that morph at runtime: character heads,
mouths, eyes, brows, beards — every FaceGen part. The standard
vertex data is empty (`Data Size` 0, 0 triangles): for skinned SSE
meshes the triangles and vertex attributes live in the shape's
`NiSkinPartition` block, not in the shape (see below). What the
shape block itself carries is the vertex *positions*, in the dynamic
section:

| Field | Size | Notes |
| --- | --- | --- |
| (all `BSTriShape` fields) | | Data Size 0, Particle Data Size 0 in every real file swept |
| Dynamic Data Size | u32 | Bytes of dynamic data: stride × Num Vertices |
| Dynamic Vertices | struct[] | One record per vertex, `stride` bytes each (vertex desc bits 4–7, × 4; 16 in every real file swept) |

Each dynamic record's first 12 bytes are the vertex position
(Vector3, full-precision float). The remaining bytes (4 per vertex:
one float, 0 on heads, in [−1, 1] on beards) are uninterpreted and
skipped; their meaning is not confirmed by the spec the reader
follows.

Verified by the M6.1 sweep over both Meshes BSAs (21,132 blocks):
every block's Dynamic Data Size equals stride × Num Vertices exactly
(0 mismatches), 0 trailing bytes after the dynamic section, and
8,179,552 positions parsed, 99.78% inside their shape's bounding
sphere (the 0.22% outside is exporter looseness in the spheres, not
a layout error — the byte-exact size check is the proof).

### Skinned SSE meshes — skin blocks (M12)

For skinned Skyrim SE meshes, the shape block does **not** hold the
geometry: the vertex data and triangles move to the shape's
`NiSkinPartition` block (reached via the skin instance), and the
shape's own counts are zeroed. `BSDynamicTriShape` is the FaceGen
case (positions in the dynamic section, everything else in the
partition); skinned `BSTriShape`s are the same pattern without the
dynamic section. The loader parses all four skin blocks and
reconstructs these meshes in bind pose. Scale from the M12 sweep:
26,942 shapes (21,132 `BSDynamicTriShape` + 5,810 `BSTriShape`
with a skin instance) across both Meshes BSAs — every character,
creature, armor, and clothing mesh — all assemble, 0 gaps, 0
trailing bytes in any skin block.

All three block types inherit `NiObject`, NOT `NiObjectNET`: no
name / extra-data / controller header.

#### NiSkinInstance / BSDismemberSkinInstance (SE layout)

| Field | Size | Notes |
| --- | --- | --- |
| Data | Ref | `NiSkinData` |
| Skin Partition | Ref | `NiSkinPartition` — the SE addition; old docs showing Data/Root/Bones are wrong for SE |
| Root | Ref | Skeleton root node |
| Num Bones | u32 | |
| Bones | Ref[] | Bone nodes, in `NiSkinData` bone-list order |
| (dismember only) Num Partitions | u32 | |
| (dismember only) Partitions | (u16 flags, u16 part id)[] | Dismember body-part IDs; parsed, not used for bind pose |

#### NiSkinData (SE layout)

| Field | Size | Notes |
| --- | --- | --- |
| Skin Transform | SkinTransform | Rotation (Mat3), Translation (Vec3), Scale (f32) — rotation FIRST, unlike AvObject |
| Num Bones | u32 | |
| Unknown byte | u8 | 0x01 in every file checked; purpose unknown |
| Per bone: Skin Transform, Bounding Sphere (Vec3 + f32), Num Vertices (u16), Vertex Weights (u16 index + f32 weight)[] | | LE-style weights; SE bind pose uses the partition's weights instead |

There is NO trailing skin-partition ref in SE — the block ends
after the last bone's weights (the LE layout's trailing ref is
absent; do not read one). The block ends exactly (0 trailing
bytes) in every file swept.

#### NiSkinPartition (SE layout)

| Field | Size | Notes |
| --- | --- | --- |
| Num Partitions | u32 | |
| Vertex Data Size | u32 | Total bytes of the shape's FULL vertex array |
| Vertex Size | u32 | Stride |
| Vertex Desc | u64 | `BSVertexDesc`, same value as the shape's |
| Vertex Data | bytes | `BSVertexDataSSE` records, the shape's full array stored ONCE (not per-partition) |
| Per partition: | | |
| Num Vertices / Num Triangles / Num Bones / Num Strips / Num Weights Per Vertex | u16 each | |
| Bones | u16[] | Indices into the skin instance's bone list |
| Has Vertex Map (u8), Vertex Map | u16[] | Partition-local vertex → global vertex index |
| Has Vertex Weights (u8), Vertex Weights | f32[][] | Per-local-vertex, per-weight |
| Strip Lengths | u16[] | |
| Has Faces (u8), Triangles | u16[][3] | or strips when Num Strips > 0 |
| Has Bone Indices (u8), Bone Indices | u8[][] | Per-local-vertex, per-weight (slots into the partition's Bones) |
| Unknown Short | u16 | 0 in all files checked |
| Vertex Desc (repeat) | u64 | Identical to the header vdesc, every time |
| Triangles (duplicate) | u16[][3] | Byte-identical to the struct's triangles; parsed once, duplicate skipped |

Two facts that are easy to get wrong:

- **Triangles index GLOBAL vertex indices.** The body's partition
  part 0 has 1,275 local verts but its triangles reach index 1,384
  = global count − 1. The vertex map maps partition-local *weight*
  entries to global vertices; triangles need no remapping.
- **Partitions overlap at dismember seams.** The body has 1,429
  local verts across 3 partitions vs 1,385 global. The assembler
  takes the first partition's weights per global vertex.

#### Bind-pose reconstruction

Per vertex, `v' = Σ_b w_b × (BoneWorld_b ∘ BoneSkin_b ∘ GeomSkin)
× v`, where `BoneWorld` is the bone node's composed scene-graph
transform, `BoneSkin` is the `NiSkinData` bone transform, and
`GeomSkin` is the `NiSkinData` geometry transform. The shape's own
composed transform is NOT baked in: it stays on `Mesh.transform`
and the renderer applies it exactly once (linearity makes the two
identical). Normals, tangents, and bitangents get the same weighted
treatment, rotation-only, renormalized — required because
thigh/calf bind rotations are ~180° about Y and flip normals.
Vertices with no weights fall back to the untransformed position;
weights normalize by their sum defensively.

Numerically proven on `MaleBody_1.nif` and the FaceGen heads
(Delphine/Orgnar): every part lands in character space (feet z=0,
head z≈120–132), eyes above mouth, brows above eyes. Two sanity
anchors: the head shape's GeomSkin (0, 1.55, −120.34) is exactly
the inverse of its shape transform (0, −1.55, 120.34); the mouth's
GeomSkin is identity and its single bone (Head) has identity
BoneSkin, so the mouth's placement is purely BoneWorld_Head.

LE-style skinning (`NiTriShape`/`NiTriStrips` + `NiSkinInstance`)
does NOT occur in Skyrim SE's Meshes BSAs (the M6.1 sweep found
26,980 skinned shapes = 21,132 `BSDynamicTriShape` + 5,848
`BSTriShape`, zero `NiTriShape`), so those shapes keep their
inline vertices with no skinning. `BSTriShape` with BOTH inline
vertices and a skin instance also does not occur (0 in the M12
sweep) — inline wins if it ever does.

### BSLightingShaderProperty

NiObjectNET (with the Shader Type u32 first), then:

| Field | Size | Notes |
| --- | --- | --- |
| Shader Flags 1 | u32 | `SkyrimShaderPropertyFlags1` |
| Shader Flags 2 | u32 | `SkyrimShaderPropertyFlags2` |
| UV Offset | TexCoord | |
| UV Scale | TexCoord | |
| Texture Set | Ref | `BSShaderTextureSet` |
| Emissive Color | Color3 | |
| Emissive Multiple | float | |
| Texture Clamp Mode | u32 | |
| Alpha | float | Opacity (1 = opaque) |
| Refraction Strength | float | |
| Glossiness | float | |
| Specular Color | Color3 | |
| Specular Strength | float | |
| Lighting Effect 1 | float | |
| Lighting Effect 2 | float | |
| (conditional) | | Extra fields by shader type (below) |

Shader types (`BSLightingShaderType`): 0 Default, 1 Environment Map,
2 Glow Shader, 3 Parallax, 4 Face Tint, 5 Skin Tint, 6 Hair Tint,
7 Parallax Occ, 8 Multitexture Landscape, 9 LOD Landscape, 10 Snow,
11 MultiLayer Parallax, 12 Tree Anim, 13 LOD Objects, 14 Sparkle
Snow, 15 LOD Objects HD, 16 Eye Envmap, 17 Cloud, 18 LOD Landscape
Noise, 19 Multitexture Landscape LOD Blend, 20 FO4 Dismemberment.

Conditional trailing fields by shader type: type 1 adds Environment
Map Scale (float); type 5 adds Skin Tint Color (Color3); type 6 adds
Hair Tint Color (Color3); type 7 adds Max Passes + Scale (floats);
type 11 adds Parallax Inner Layer Thickness, Parallax Refraction
Scale (floats), Parallax Inner Layer Texture Scale (TexCoord),
Parallax Envmap Strength (float); type 14 adds Sparkle Parameters
(Vector4); type 16 adds Eye Cubemap Scale (float) and Left/Right Eye
Reflection Center (Vector3).

Shader flag bits the renderer will care about (Flags 1): bit 1
Skinned, bit 3 Vertex Alpha, bit 7 Environment Mapping, bit 8
Receive Shadows, bit 9 Cast Shadows, bit 14 Landscape, bit 15
Refraction. (Flags 2): bit 4 Double Sided, bit 5 Vertex Colors, bit 6
Glow Map, bit 19 Premult Alpha, bit 24 Multi Layer Parallax, bit 26
Rim Lighting, bit 27 Back Lighting, bit 29 Tree Anim.

### BSShaderTextureSet

Num Textures (u32), then that many `SizedString` texture paths.
Slots: 0 diffuse, 1 normal/gloss, 2 glow / skin / hair / rim light,
3 height/parallax, 4 environment, 5 environment mask, 6 subsurface
(multilayer parallax), 7 backlighting. Paths use backslashes
(`textures\...`) and name files inside the BSAs.

### NiTexturingProperty — the legacy texture path

NiObjectNET, then Flags (u16), Texture Count (u32, default 7), then
one bool + `TexDesc` per slot: Base, Dark, Detail, Gloss, Glow; Bump
Map (only when Texture Count > 5, plus Luma Scale, Luma Offset, and
a Matrix22) ; Normal (only when > 6); Parallax (only when > 7, plus
a Parallax Offset float); Decal 0–3 (only when > 8/9/10/11); then Num
Shader Textures (u32) + Shader Textures (`ShaderTexDesc`: Has Map
bool, Map TexDesc, Map ID u32).

`TexDesc`: Source (Ref to `NiSourceTexture`), Flags (u16), Has
Texture Transform (bool), and if set: Translation (TexCoord), Scale
(TexCoord), Rotation (float), Transform Method (u32), Center
(TexCoord).

In Skyrim, textures normally come through `BSLightingShaderProperty`
+ `BSShaderTextureSet`, not through `NiTexturingProperty` (Skyrim
shapes have no property list to attach one to). The reader parses
`NiTexturingProperty` when present, but mesh assembly uses the
shader-property path.

### NiSourceTexture

NiObjectNET, then Use External (byte), File Name (FilePath — the
texture path when external, the original source filename when the
pixels are embedded), Pixel Data (Ref), Format Prefs (3 × u32:
pixel layout, mipmap use, alpha format — renderer hints only),
Is Static (byte), Direct Render (bool), Persist Render Data (bool).

### NiAlphaProperty / NiMaterialProperty

`NiAlphaProperty`: NiObjectNET, Flags (u16), Threshold (byte).
`NiMaterialProperty` (Skyrim): NiObjectNET, Specular Color (Color3),
Emissive Color (Color3), Glossiness (float), Alpha (float), Emissive
Mult (float). Ambient and Diffuse exist only for BS version < 26.

## Animation, collision, particles — future work

Not parsed; their blocks are skipped by size:

- **Animation:** `NiControllerManager`, `NiKeyframeController`,
  `NiTransformController`, `NiControllerSequence`, and the `.kf`
  files that carry animation keys. Needed for anything that moves.
  (Skinning itself is done — see the skin-blocks section above;
  bind pose only, no animation yet.)
- **Collision:** `bhkCollisionObject` and friends (Havok). Needed
  for walking on the world; M6 work.
- **Particles:** `NiParticleSystem`, `BSMasterParticleSystem`.

## Reader notes (dovah-nif)

- Supports NIF 20.2.0.7 with BS version 83 (Skyrim LE) or 100
  (Skyrim SE). Other versions are rejected — including 6 legacy
  files in the Meshes BSAs with BS stream 32/34 (Oblivion-era
  leftovers: two ferns, two test-white meshes, two test swords).
- Blocks are walked by their header block sizes; unknown types are
  skipped, so a file full of skinning/animation/collision blocks
  still yields its meshes.
- `NifFile::meshes()` assembles plain mesh data: vertices, normals,
  UVs, tangents, bitangents, vertex colors, triangle indices,
  material name, texture paths, bounding sphere, and the shape's
  COMPOSED scene-graph transform (M11: every parent node's
  transform composed down to the shape). Only drawable meshes are
  returned — shapes with no vertices or no indices are skipped.
  Skinned shapes are reconstructed in bind pose from their skin
  partition (M12), so they assemble like any other mesh.
- Material name comes from the shape's Material Data (active
  material) for Ni* shapes, and from the shader property's name for
  `BSTriShape` (which has no Material Data).
- Leftover bytes inside a parsed block are ignored (lenient);
  truncation anywhere is an error.
- Real-data check DONE (M6.1, `examples/nif_sweep.rs`): 21,827
  `.nif` files across both Meshes BSAs, 21,821 parsed cleanly (0
  extract failures, 0 panics), 53,767 drawable meshes assembled
  (23,261,712 vertices, 23,137,329 triangles). The 6 failures are
  the BS 32/34 legacy files above. 7,145 files yield no drawable
  mesh yet — all skinned shapes waiting on the skin-partition
  reconstruction.
