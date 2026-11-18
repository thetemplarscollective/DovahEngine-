//! Round-trip tests for `dovah-nif`: build synthetic NIF files from
//! the spec (NifTools `nif.xml`), parse them back, and check the
//! assembled meshes.

use dovah_nif::{
    alpha_test_cutoff, Block, CollisionShape, Mesh, NifError, NifFile, ShaderTypeExtra,
};

// ---------------------------------------------------------------------------
// Byte writer + file builder
// ---------------------------------------------------------------------------

struct W {
    buf: Vec<u8>,
}

impl W {
    fn new() -> Self {
        W { buf: Vec::new() }
    }

    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }

    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Write raw half-float bits.
    fn hfloat(&mut self, bits: u16) {
        self.u16(bits);
    }

    fn vec2(&mut self, v: [f32; 2]) {
        for x in v {
            self.f32(x);
        }
    }

    fn vec3(&mut self, v: [f32; 3]) {
        for x in v {
            self.f32(x);
        }
    }

    fn vec4(&mut self, v: [f32; 4]) {
        for x in v {
            self.f32(x);
        }
    }

    fn mat3_identity(&mut self) {
        for x in [1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
            self.f32(x);
        }
    }

    fn sized_string(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
    }

    fn export_string(&mut self, s: &str) {
        self.u8((s.len() + 1) as u8);
        self.buf.extend_from_slice(s.as_bytes());
        self.u8(0);
    }

    fn triangle(&mut self, t: [u16; 3]) {
        for v in t {
            self.u16(v);
        }
    }
}

/// Half-float bits for the few values the fixtures use.
fn f16_bits(value: f32) -> u16 {
    match value {
        0.0 => 0x0000,
        0.5 => 0x3800,
        1.0 => 0x3C00,
        -1.0 => 0xBC00,
        _ => panic!("f16_bits: unsupported test value {value}"),
    }
}

struct Builder {
    strings: Vec<String>,
    blocks: Vec<(String, Vec<u8>)>,
}

impl Builder {
    fn new() -> Self {
        Builder {
            strings: Vec::new(),
            blocks: Vec::new(),
        }
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(i) = self.strings.iter().position(|x| x == s) {
            return i as u32;
        }
        self.strings.push(s.to_string());
        (self.strings.len() - 1) as u32
    }

    fn add(&mut self, type_name: &str, bytes: Vec<u8>) -> i32 {
        self.blocks.push((type_name.to_string(), bytes));
        (self.blocks.len() - 1) as i32
    }

    fn finish(self, bs_version: u32) -> Vec<u8> {
        let mut w = W::new();
        w.buf
            .extend_from_slice(b"Gamebryo File Format, Version 20.2.0.7\n");
        w.u32(0x1402_0007);
        w.u8(1); // little-endian
        w.u32(12); // user version
        w.u32(self.blocks.len() as u32);
        // BS stream header.
        w.u32(bs_version);
        w.export_string("Dovahengine test");
        w.export_string(""); // process script
        w.export_string(""); // export script
                             // Block types, deduplicated in first-use order.
        let mut types: Vec<String> = Vec::new();
        let mut type_index = Vec::new();
        for (t, _) in &self.blocks {
            let idx = match types.iter().position(|x| x == t) {
                Some(i) => i,
                None => {
                    types.push(t.clone());
                    types.len() - 1
                }
            };
            type_index.push(idx as u16);
        }
        w.u16(types.len() as u16);
        for t in &types {
            w.sized_string(t);
        }
        for i in type_index {
            w.u16(i);
        }
        for (_, bytes) in &self.blocks {
            w.u32(bytes.len() as u32);
        }
        w.u32(self.strings.len() as u32);
        w.u32(
            self.strings
                .iter()
                .map(|s| s.len() as u32)
                .max()
                .unwrap_or(0),
        );
        for s in &self.strings {
            w.sized_string(s);
        }
        w.u32(0); // num groups
        for (_, bytes) in &self.blocks {
            w.buf.extend_from_slice(bytes);
        }
        // Footer: one root, block 0.
        w.u32(1);
        w.i32(0);
        w.buf
    }
}

// ---------------------------------------------------------------------------
// Block writers (field order per nif.xml, Skyrim SE)
// ---------------------------------------------------------------------------

fn write_net(w: &mut W, b: &mut Builder, name: &str, shader_type: Option<u32>) {
    if let Some(t) = shader_type {
        w.u32(t);
    }
    w.u32(b.intern(name));
    w.u32(0); // num extra data
    w.i32(-1); // controller
}

fn write_av(w: &mut W, b: &mut Builder, name: &str, translation: [f32; 3], scale: f32) {
    write_net(w, b, name, None);
    w.u32(0x8000E); // flags
    w.vec3(translation);
    w.mat3_identity();
    w.f32(scale);
    w.i32(-1); // collision object
}

fn write_material_data(w: &mut W, b: &mut Builder, names: &[&str], active: i32) {
    w.u32(names.len() as u32);
    for n in names {
        w.u32(b.intern(n));
    }
    for _ in names {
        w.i32(-1);
    }
    w.i32(active);
    w.bool(false);
}

#[allow(clippy::too_many_arguments)]
fn write_geometry(
    w: &mut W,
    b: &mut Builder,
    name: &str,
    data: i32,
    shader: i32,
    alpha: i32,
    materials: &[&str],
    active: i32,
    translation: [f32; 3],
    scale: f32,
) {
    write_av(w, b, name, translation, scale);
    w.i32(data);
    w.i32(-1); // skin instance
    write_material_data(w, b, materials, active);
    w.i32(shader);
    w.i32(alpha);
}

/// A (tangents, bitangents) pair for `write_geometry_data`.
type TangentPair<'a> = (&'a [[f32; 3]], &'a [[f32; 3]]);

#[allow(clippy::too_many_arguments)]
fn write_geometry_data(
    w: &mut W,
    vertices: &[[f32; 3]],
    normals: &[[f32; 3]],
    tangents: Option<TangentPair<'_>>,
    colors: Option<&[[f32; 4]]>,
    uvs: Option<&[[f32; 2]]>,
) {
    w.i32(0); // group ID
    w.u16(vertices.len() as u16);
    w.u8(0); // keep flags
    w.u8(0); // compress flags
    w.bool(true); // has vertices
    for v in vertices {
        w.vec3(*v);
    }
    let mut flags: u16 = 0;
    if uvs.is_some() {
        flags |= 1;
    }
    if tangents.is_some() {
        flags |= 0x1000;
    }
    w.u16(flags); // BS data flags
    w.u32(0); // material CRC
    w.bool(!normals.is_empty());
    for n in normals {
        w.vec3(*n);
    }
    if let Some((t, bt)) = tangents {
        for v in t {
            w.vec3(*v);
        }
        for v in bt {
            w.vec3(*v);
        }
    }
    w.vec3([0.0, 0.0, 0.0]); // bounding sphere center
    w.f32(1.0); // bounding sphere radius
    w.bool(colors.is_some());
    if let Some(c) = colors {
        for v in c {
            w.vec4(*v);
        }
    }
    if let Some(u) = uvs {
        for v in u {
            w.vec2(*v);
        }
    }
    w.u16(0x4000); // consistency flags: static
    w.i32(-1); // additional data
}

/// Write a BS tri-shape body (BSTriShape / BSDynamicTriShape share
/// the layout): one triangle, Vertex+UVs+Normals, plus the
/// half-precision particle copy. Vertex Desc: 24 bytes/vertex
/// (nibble 6), attributes 0xB << 44.
fn write_bs_tri_shape(w: &mut W, b: &mut Builder, name: &str, shader: i32) {
    write_av(w, b, name, [0.0, 0.0, 0.0], 1.0);
    w.vec3([0.0, 0.0, 0.0]); // bounding sphere center
    w.f32(1.0); // bounding sphere radius
    w.i32(-1); // skin
    w.i32(shader); // shader property
    w.i32(-1); // alpha property
    let desc: u64 = 6 | (0xB << 44);
    w.u64(desc);
    w.u16(1); // num triangles
    w.u16(3); // num vertices
    w.u32(6 * 3 * 4 + 6); // data size: (desc & 0xF) * 3 verts * 4 + 1 tri * 6
                          // Vertices: position, unused W, half UV, byte normal, bitangent Y.
    let bs_verts: [([f32; 3], [u16; 2], [i8; 3]); 3] = [
        ([0.0, 0.0, 0.0], [0x0000, 0x0000], [0, 0, 127]),
        ([1.0, 0.0, 0.0], [0x3C00, 0x0000], [0, 0, 127]),
        ([0.0, 1.0, 0.0], [0x3800, 0x3C00], [0, 0, 127]),
    ];
    for (pos, uv, nrm) in &bs_verts {
        w.vec3(*pos);
        w.u32(0); // unused W
        w.hfloat(uv[0]);
        w.hfloat(uv[1]);
        for byte in nrm {
            w.u8(*byte as u8);
        }
        w.u8(0); // bitangent Y
    }
    w.triangle([0, 1, 2]);
    // Particle data: half-precision copy of the same triangle.
    w.u32(3 * 6 + 3); // particle data size: 3 verts * 6 + 1 tri * 3
    for (pos, _, _) in &bs_verts {
        for x in pos {
            w.hfloat(f16_bits(*x));
        }
    }
    for _ in 0..3 {
        w.hfloat(0x0000);
        w.hfloat(0x0000);
        w.hfloat(0x3C00);
    }
    w.triangle([0, 1, 2]);
}

/// Build the full SE test file. Block layout:
/// 0 NiNode, 1 NiTriShape, 2 NiTriShapeData, 3 BSLightingShaderProperty,
/// 4 BSTriShape, 5 BSShaderTextureSet, 6 NiTriStrips, 7 NiTriStripsData,
/// 8 NiAlphaProperty, 9 NiSourceTexture, 10 NiTexturingProperty,
/// 11 NiMaterialProperty, 12 unknown block, 13 BSLODTriShape,
/// 14 BSDynamicTriShape.
fn build_se_nif() -> Vec<u8> {
    let mut b = Builder::new();

    // 0: NiNode "Scene Root", children 1, 4, 6, 13.
    let mut w = W::new();
    write_av(&mut w, &mut b, "Scene Root", [0.0, 0.0, 0.0], 1.0);
    w.u32(4);
    for child in [1, 4, 6, 13] {
        w.i32(child);
    }
    w.u32(0); // num effects
    assert_eq!(b.add("NiNode", w.buf), 0);

    // 1: NiTriShape "TestShape", data 2, shader 3, alpha 8.
    let mut w = W::new();
    write_geometry(
        &mut w,
        &mut b,
        "TestShape",
        2,
        3,
        8,
        &["TestMaterial"],
        0,
        [1.0, 2.0, 3.0],
        2.0,
    );
    assert_eq!(b.add("NiTriShape", w.buf), 1);

    // 2: NiTriShapeData: one triangle with normals, tangents, colors, UVs.
    let mut w = W::new();
    let verts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let normals = [[0.0, 0.0, 1.0]; 3];
    let tangents = [[1.0, 0.0, 0.0]; 3];
    let bitangents = [[0.0, 1.0, 0.0]; 3];
    let colors = [[1.0, 1.0, 1.0, 1.0]; 3];
    let uvs = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
    write_geometry_data(
        &mut w,
        &verts,
        &normals,
        Some((&tangents, &bitangents)),
        Some(&colors),
        Some(&uvs),
    );
    w.u16(1); // num triangles
    w.u32(3); // num triangle points
    w.bool(true); // has triangles
    w.triangle([0, 1, 2]);
    w.u16(1); // num match groups
    w.u16(2); // group size
    w.u16(0);
    w.u16(1);
    assert_eq!(b.add("NiTriShapeData", w.buf), 2);

    // 3: BSLightingShaderProperty "TestShader", type 0, texture set 5.
    let mut w = W::new();
    write_net(&mut w, &mut b, "TestShader", Some(0));
    w.u32(0x8240_0301); // shader flags 1
    w.u32(0x8021); // shader flags 2
    w.vec2([0.0, 0.0]); // UV offset
    w.vec2([1.0, 1.0]); // UV scale
    w.i32(5); // texture set
    w.vec3([0.0, 0.0, 0.0]); // emissive color
    w.f32(1.0); // emissive multiple
    w.u32(3); // texture clamp mode (wrap S wrap T)
    w.f32(1.0); // alpha
    w.f32(0.0); // refraction strength
    w.f32(80.0); // glossiness
    w.vec3([1.0, 1.0, 1.0]); // specular color
    w.f32(1.0); // specular strength
    w.f32(0.3); // lighting effect 1
    w.f32(2.0); // lighting effect 2
                // No extra fields for shader type 0.
    assert_eq!(b.add("BSLightingShaderProperty", w.buf), 3);

    // 4: BSTriShape "BsShape", shader 3.
    let mut w = W::new();
    write_bs_tri_shape(&mut w, &mut b, "BsShape", 3);
    assert_eq!(b.add("BSTriShape", w.buf), 4);

    // 5: BSShaderTextureSet with 6 textures.
    let mut w = W::new();
    let textures = [
        "textures\\test_d.dds",
        "textures\\test_n.dds",
        "textures\\test_g.dds",
        "textures\\test_h.dds",
        "textures\\test_e.dds",
        "textures\\test_m.dds",
    ];
    w.u32(textures.len() as u32);
    for t in textures {
        w.sized_string(t);
    }
    assert_eq!(b.add("BSShaderTextureSet", w.buf), 5);

    // 6: NiTriStrips "StripShape", data 7, shader 3, no materials.
    let mut w = W::new();
    write_geometry(
        &mut w,
        &mut b,
        "StripShape",
        7,
        3,
        -1,
        &[],
        -1,
        [0.0, 0.0, 0.0],
        1.0,
    );
    assert_eq!(b.add("NiTriStrips", w.buf), 6);

    // 7: NiTriStripsData: a quad as one 4-point strip, no normals/UVs.
    let mut w = W::new();
    let strip_verts = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    write_geometry_data(&mut w, &strip_verts, &[], None, None, None);
    w.u16(2); // num triangles
    w.u16(1); // num strips
    w.u16(4); // strip length
    w.bool(true); // has points
    for p in [0u16, 1, 2, 3] {
        w.u16(p);
    }
    assert_eq!(b.add("NiTriStripsData", w.buf), 7);

    // 8: NiAlphaProperty.
    let mut w = W::new();
    write_net(&mut w, &mut b, "AlphaProp", None);
    w.u16(4844);
    w.u8(128);
    assert_eq!(b.add("NiAlphaProperty", w.buf), 8);

    // 9: NiSourceTexture, external.
    let mut w = W::new();
    write_net(&mut w, &mut b, "SrcTex", None);
    w.u8(1); // use external
    w.u32(b.intern("textures\\legacy.dds"));
    w.i32(-1); // pixel data
    w.u32(0); // pixel layout
    w.u32(0); // use mipmaps
    w.u32(0); // alpha format
    w.u8(1); // is static
    w.bool(true); // direct render
    w.bool(false); // persist render data
    assert_eq!(b.add("NiSourceTexture", w.buf), 9);

    // 10: NiTexturingProperty, texture count 7, base texture only.
    let mut w = W::new();
    write_net(&mut w, &mut b, "TexProp", None);
    w.u16(0); // flags
    w.u32(7); // texture count
    w.bool(true); // has base texture
    w.i32(9); // source
    w.u16(0); // map flags
    w.bool(false); // has transform
    w.bool(false); // has dark
    w.bool(false); // has detail
    w.bool(false); // has gloss
    w.bool(false); // has glow
    w.bool(false); // has bump map (count > 5)
    w.bool(false); // has normal (count > 6)
                   // Parallax/decals absent (count is 7, not > 7).
    w.u32(0); // num shader textures
    assert_eq!(b.add("NiTexturingProperty", w.buf), 10);

    // 11: NiMaterialProperty.
    let mut w = W::new();
    write_net(&mut w, &mut b, "MatProp", None);
    w.vec3([1.0, 1.0, 1.0]); // specular
    w.vec3([0.0, 0.0, 0.0]); // emissive
    w.f32(10.0); // glossiness
    w.f32(1.0); // alpha
    w.f32(1.0); // emissive mult
    assert_eq!(b.add("NiMaterialProperty", w.buf), 11);

    // 12: Unknown block type, skipped by size.
    assert_eq!(b.add("NiSomeFutureBlock", vec![9, 8, 7, 6, 5]), 12);

    // 13: BSLODTriShape "LodShape", reusing data block 2.
    let mut w = W::new();
    write_geometry(
        &mut w,
        &mut b,
        "LodShape",
        2,
        3,
        -1,
        &["LodMaterial"],
        0,
        [0.0, 0.0, 0.0],
        1.0,
    );
    w.u32(3); // LOD0 size
    w.u32(2); // LOD1 size
    w.u32(1); // LOD2 size
    assert_eq!(b.add("BSLODTriShape", w.buf), 13);

    // 14: BSDynamicTriShape "DynShape", shader 3 — same body layout
    // as the BSTriShape above.
    let mut w = W::new();
    write_bs_tri_shape(&mut w, &mut b, "DynShape", 3);
    assert_eq!(b.add("BSDynamicTriShape", w.buf), 14);

    b.finish(100)
}

fn mesh_by_name<'a>(meshes: &'a [Mesh], name: &str) -> &'a Mesh {
    meshes.iter().find(|m| m.name == name).unwrap()
}

#[test]
fn parses_se_nif_and_assembles_meshes() {
    let bytes = build_se_nif();
    let nif = NifFile::from_bytes(&bytes).unwrap();

    assert_eq!(nif.version, 0x1402_0007);
    assert_eq!(nif.user_version, 12);
    assert_eq!(nif.bs_version, 100);
    assert_eq!(nif.header_string, "Gamebryo File Format, Version 20.2.0.7");
    assert_eq!(nif.blocks.len(), 15);
    assert_eq!(nif.root_indices, vec![0]);

    // The unknown block is kept, not an error.
    match &nif.blocks[12] {
        Block::Unknown { type_name } => assert_eq!(type_name, "NiSomeFutureBlock"),
        other => panic!("block 12 should be Unknown, got {:?}", other.type_name()),
    }

    // The node hierarchy.
    match &nif.blocks[0] {
        Block::Node(node) => {
            assert_eq!(node.av.net.name, "Scene Root");
            assert_eq!(node.children, vec![1, 4, 6, 13]);
            assert!(node.effects.is_empty());
        }
        other => panic!("block 0 should be a node, got {:?}", other.type_name()),
    }

    // The shader property.
    match &nif.blocks[3] {
        Block::LightingShaderProperty(shader) => {
            assert_eq!(shader.net.name, "TestShader");
            assert_eq!(shader.shader_type, 0);
            assert_eq!(shader.shader_flags_1, 0x8240_0301);
            assert_eq!(shader.shader_flags_2, 0x8021);
            assert_eq!(shader.texture_set, 5);
            assert_eq!(shader.glossiness, 80.0);
            assert!(matches!(shader.extra, ShaderTypeExtra::None));
        }
        other => panic!("block 3 should be a shader, got {:?}", other.type_name()),
    }

    // The texture set.
    match &nif.blocks[5] {
        Block::ShaderTextureSet(set) => {
            assert_eq!(set.textures.len(), 6);
            assert_eq!(set.textures[0], "textures\\test_d.dds");
            assert_eq!(set.textures[1], "textures\\test_n.dds");
        }
        other => panic!(
            "block 5 should be a texture set, got {:?}",
            other.type_name()
        ),
    }

    // The legacy texturing property + source texture.
    match &nif.blocks[10] {
        Block::TexturingProperty(prop) => {
            assert_eq!(prop.texture_count, 7);
            assert_eq!(prop.base.as_ref().unwrap().source, 9);
            assert!(prop.dark.is_none());
            assert!(prop.bump_map.is_none());
        }
        other => panic!(
            "block 10 should be a texturing property, got {:?}",
            other.type_name()
        ),
    }
    match &nif.blocks[9] {
        Block::SourceTexture(tex) => {
            assert_eq!(tex.use_external, 1);
            assert_eq!(tex.file_name, "textures\\legacy.dds");
        }
        other => panic!(
            "block 9 should be a source texture, got {:?}",
            other.type_name()
        ),
    }

    // Alpha + material properties.
    match &nif.blocks[8] {
        Block::AlphaProperty(alpha) => {
            assert_eq!(alpha.flags, 4844);
            assert_eq!(alpha.threshold, 128);
        }
        other => panic!("block 8 should be alpha, got {:?}", other.type_name()),
    }
    match &nif.blocks[11] {
        Block::MaterialProperty(mat) => assert_eq!(mat.glossiness, 10.0),
        other => panic!("block 11 should be material, got {:?}", other.type_name()),
    }

    // The tri-shape data: match groups parsed.
    match &nif.blocks[2] {
        Block::TriShapeData(data) => {
            assert_eq!(data.num_triangles, 1);
            assert_eq!(data.triangles, vec![[0, 1, 2]]);
            assert_eq!(data.match_groups, vec![vec![0, 1]]);
        }
        other => panic!("block 2 should be tri data, got {:?}", other.type_name()),
    }

    // The BS tri shape: particle copy parsed.
    match &nif.blocks[4] {
        Block::BsTriShape(shape) => {
            assert_eq!(shape.vertices.len(), 3);
            assert_eq!(shape.particle_vertices.len(), 3);
            assert_eq!(shape.particle_vertices[1], [1.0, 0.0, 0.0]);
            assert_eq!(shape.particle_normals[0], [0.0, 0.0, 1.0]);
            assert_eq!(shape.vertex_desc, 6 | (0xB << 44));
        }
        other => panic!("block 4 should be a BS shape, got {:?}", other.type_name()),
    }

    // The LOD shape.
    match &nif.blocks[13] {
        Block::BsLodTriShape(shape) => assert_eq!(shape.lod_sizes, [3, 2, 1]),
        other => panic!(
            "block 13 should be an LOD shape, got {:?}",
            other.type_name()
        ),
    }

    // The dynamic tri shape: same layout as BSTriShape, no trailing
    // bytes, no dynamic section (inline data instead).
    match &nif.blocks[14] {
        Block::BsDynamicTriShape(dynamic) => {
            assert_eq!(dynamic.trailing_bytes, 0);
            assert_eq!(dynamic.dynamic_data_size, 0);
            assert!(dynamic.dynamic_positions.is_empty());
            assert_eq!(dynamic.shape.num_vertices, 3);
            assert_eq!(dynamic.shape.num_triangles, 1);
            assert_eq!(dynamic.shape.vertices.len(), 3);
            assert_eq!(dynamic.shape.particle_vertices.len(), 3);
            assert_eq!(dynamic.shape.vertex_desc, 6 | (0xB << 44));
        }
        other => panic!(
            "block 14 should be a dynamic shape, got {:?}",
            other.type_name()
        ),
    }

    // Mesh assembly: 5 shapes.
    let meshes = nif.meshes();
    assert_eq!(meshes.len(), 5);

    let test = mesh_by_name(&meshes, "TestShape");
    assert_eq!(
        test.vertices,
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
    );
    assert_eq!(test.normals, vec![[0.0, 0.0, 1.0]; 3]);
    assert_eq!(test.tangents, vec![[1.0, 0.0, 0.0]; 3]);
    assert_eq!(test.bitangents, vec![[0.0, 1.0, 0.0]; 3]);
    assert_eq!(test.vertex_colors, vec![[1.0, 1.0, 1.0, 1.0]; 3]);
    assert_eq!(test.uvs, vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
    assert_eq!(test.indices, vec![0, 1, 2]);
    assert_eq!(test.material_name, "TestMaterial");
    assert_eq!(
        test.textures.diffuse.as_deref(),
        Some("textures\\test_d.dds")
    );
    assert_eq!(
        test.textures.normal.as_deref(),
        Some("textures\\test_n.dds")
    );
    assert_eq!(test.textures.glow.as_deref(), Some("textures\\test_g.dds"));
    assert_eq!(
        test.textures.height.as_deref(),
        Some("textures\\test_h.dds")
    );
    assert_eq!(
        test.textures.environment.as_deref(),
        Some("textures\\test_e.dds")
    );
    assert_eq!(
        test.textures.environment_mask.as_deref(),
        Some("textures\\test_m.dds")
    );
    assert_eq!(test.bounding_sphere, Some(([0.0, 0.0, 0.0], 1.0)));
    assert_eq!(test.transform.translation, [1.0, 2.0, 3.0]);
    assert_eq!(test.transform.scale, 2.0);

    let bs = mesh_by_name(&meshes, "BsShape");
    assert_eq!(
        bs.vertices,
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
    );
    // Byte normals (0, 0, 127) decode to (0, 0, 1).
    assert_eq!(bs.normals, vec![[0.0, 0.0, 1.0]; 3]);
    // Half-float UVs, including 0.5 = 0x3800.
    assert_eq!(bs.uvs, vec![[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]]);
    assert!(bs.tangents.is_empty());
    assert_eq!(bs.indices, vec![0, 1, 2]);
    // BSTriShape has no Material Data: the material name falls back
    // to the shader property's name.
    assert_eq!(bs.material_name, "TestShader");
    assert_eq!(bs.textures.diffuse.as_deref(), Some("textures\\test_d.dds"));

    let strip = mesh_by_name(&meshes, "StripShape");
    assert_eq!(strip.vertices.len(), 4);
    // One 4-point strip triangulates to 2 triangles, second flipped.
    assert_eq!(strip.indices, vec![0, 1, 2, 2, 1, 3]);
    assert!(strip.normals.is_empty());
    assert!(strip.uvs.is_empty());
    // No materials on the shape: shader name fallback.
    assert_eq!(strip.material_name, "TestShader");

    let lod = mesh_by_name(&meshes, "LodShape");
    assert_eq!(lod.indices, vec![0, 1, 2]);
    assert_eq!(lod.material_name, "LodMaterial");

    // The dynamic shape assembles exactly like the BS shape.
    let dynamic = mesh_by_name(&meshes, "DynShape");
    assert_eq!(dynamic.vertices, bs.vertices);
    assert_eq!(dynamic.normals, bs.normals);
    assert_eq!(dynamic.uvs, bs.uvs);
    assert_eq!(dynamic.indices, vec![0, 1, 2]);
    assert_eq!(dynamic.material_name, "TestShader");
    assert_eq!(
        dynamic.textures.diffuse.as_deref(),
        Some("textures\\test_d.dds")
    );
}

/// Write a BSDynamicTriShape body in the real FaceGen layout: empty
/// standard vertex data (`data_size` 0, 0 triangles), then a dynamic
/// section with one 16-byte record per position (position + one
/// uninterpreted float, here 99.0 to prove it is skipped, not read
/// as a coordinate).
fn write_dynamic_shape(w: &mut W, b: &mut Builder, name: &str, positions: &[[f32; 3]]) {
    write_av(w, b, name, [0.0, 0.0, 0.0], 1.0);
    w.vec3([0.0, 0.0, 0.0]); // bounding sphere center
    w.f32(10.0); // bounding sphere radius
    w.i32(-1); // skin
    w.i32(-1); // shader property
    w.i32(-1); // alpha property
               // Vertex desc: vertex size nibble 5, dynamic size nibble 4
               // (16 bytes/record), attribute bits 0x462 (no position bit —
               // positions live in the dynamic section).
    let desc: u64 = 5 | (4 << 4) | (0x462 << 44);
    w.u64(desc);
    w.u16(0); // num triangles (in the skin partition, not here)
    w.u16(positions.len() as u16); // num vertices
    w.u32(0); // data size (empty standard vertex data)
    w.u32(0); // particle data size
    w.u32((positions.len() * 16) as u32); // dynamic data size
    for pos in positions {
        w.vec3(*pos);
        w.f32(99.0); // uninterpreted 4th float
    }
}

#[test]
fn parses_dynamic_section() {
    let mut b = Builder::new();
    let mut w = W::new();
    let positions = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]];
    write_dynamic_shape(&mut w, &mut b, "DynHead", &positions);
    assert_eq!(b.add("BSDynamicTriShape", w.buf), 0);
    let bytes = b.finish(100);

    let nif = NifFile::from_bytes(&bytes).unwrap();
    assert_eq!(nif.blocks.len(), 1);
    match &nif.blocks[0] {
        Block::BsDynamicTriShape(dynamic) => {
            assert_eq!(dynamic.shape.num_vertices, 2);
            assert_eq!(dynamic.shape.num_triangles, 0);
            assert!(dynamic.shape.vertices.is_empty());
            assert_eq!(dynamic.dynamic_data_size, 32);
            assert_eq!(dynamic.dynamic_positions, positions);
            assert_eq!(dynamic.trailing_bytes, 0);
        }
        other => panic!(
            "block 0 should be a dynamic shape, got {:?}",
            other.type_name()
        ),
    }
    // No drawable geometry: positions without triangles are skipped
    // by meshes() until the skin-partition reconstruction lands.
    assert!(nif.meshes().is_empty());
}

#[test]
fn parses_le_nif() {
    // Skyrim LE (BS version 83): a minimal node + shape + data file.
    let mut b = Builder::new();
    let mut w = W::new();
    write_av(&mut w, &mut b, "Root", [0.0, 0.0, 0.0], 1.0);
    w.u32(1);
    w.i32(1);
    w.u32(0);
    assert_eq!(b.add("NiNode", w.buf), 0);

    let mut w = W::new();
    write_geometry(
        &mut w,
        &mut b,
        "LeShape",
        2,
        -1,
        -1,
        &[],
        -1,
        [0.0, 0.0, 0.0],
        1.0,
    );
    assert_eq!(b.add("NiTriShape", w.buf), 1);

    let mut w = W::new();
    let verts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    write_geometry_data(&mut w, &verts, &[], None, None, None);
    w.u16(1);
    w.u32(3);
    w.bool(true);
    w.triangle([0, 1, 2]);
    w.u16(0);
    assert_eq!(b.add("NiTriShapeData", w.buf), 2);

    let bytes = b.finish(83);
    let nif = NifFile::from_bytes(&bytes).unwrap();
    assert_eq!(nif.bs_version, 83);
    let meshes = nif.meshes();
    assert_eq!(meshes.len(), 1);
    assert_eq!(meshes[0].name, "LeShape");
    assert_eq!(meshes[0].indices, vec![0, 1, 2]);
    assert_eq!(meshes[0].material_name, "");
}

#[test]
fn opens_from_disk() {
    let bytes = build_se_nif();
    let path = std::env::temp_dir().join("dovah-nif-test.nif");
    std::fs::write(&path, &bytes).unwrap();
    let nif = NifFile::open(&path).unwrap();
    assert_eq!(nif.blocks.len(), 15);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn rejects_bad_magic() {
    let err = NifFile::from_bytes(b"not a nif file\nmore bytes here").unwrap_err();
    assert!(matches!(err, NifError::NotANif));
}

#[test]
fn rejects_truncated() {
    let bytes = build_se_nif();
    let half = bytes.len() / 2;
    assert!(NifFile::from_bytes(&bytes[..half]).is_err());
    assert!(NifFile::from_bytes(&[]).is_err());
}

#[test]
fn rejects_wrong_version() {
    let mut bytes = build_se_nif();
    // The version u32 sits right after the 39-byte header string.
    bytes[39..43].copy_from_slice(&0x1400_0004u32.to_le_bytes());
    let err = NifFile::from_bytes(&bytes).unwrap_err();
    assert!(matches!(
        err,
        NifError::UnsupportedVersion {
            version: 0x1400_0004,
            ..
        }
    ));
}

#[test]
fn rejects_bad_string_index() {
    // A node whose name index (99) is out of range of an empty string
    // table.
    let mut b = Builder::new();
    let mut w = W::new();
    w.u32(99); // name index, out of range
    w.u32(0); // num extra data
    w.i32(-1); // controller
    w.u32(0x8000E); // flags
    w.vec3([0.0, 0.0, 0.0]);
    w.mat3_identity();
    w.f32(1.0);
    w.i32(-1); // collision object
    w.u32(0); // num children
    w.u32(0); // num effects
    b.add("NiNode", w.buf);
    let bytes = b.finish(100);
    let err = NifFile::from_bytes(&bytes).unwrap_err();
    assert!(matches!(err, NifError::InvalidData(_)));
}

/// Write a node block (NiNode layout): AvObject + children + effects.
fn write_node(
    w: &mut W,
    b: &mut Builder,
    name: &str,
    translation: [f32; 3],
    scale: f32,
    children: &[i32],
) {
    write_av(w, b, name, translation, scale);
    w.u32(children.len() as u32);
    for child in children {
        w.i32(*child);
    }
    w.u32(0); // num effects
}

#[test]
fn parent_transform_composes_into_mesh() {
    // M11 door bug, reduced: a shape under a translated, scaled
    // parent node. The mesh transform must be parent ∘ local.
    // Block layout: 0 root NiNode (identity), 1 child NiNode
    // "Door" t=(48,4,88) scale 2, 2 NiTriShape "Panel" local
    // t=(10,0,0), 3 NiTriShapeData.
    let mut b = Builder::new();
    let mut w = W::new();
    write_node(&mut w, &mut b, "Root", [0.0, 0.0, 0.0], 1.0, &[1]);
    assert_eq!(b.add("NiNode", w.buf), 0);
    let mut w = W::new();
    write_node(&mut w, &mut b, "Door", [48.0, 4.0, 88.0], 2.0, &[2]);
    assert_eq!(b.add("NiNode", w.buf), 1);
    let mut w = W::new();
    write_geometry(
        &mut w,
        &mut b,
        "Panel",
        3,
        -1,
        -1,
        &[],
        -1,
        [10.0, 0.0, 0.0],
        1.0,
    );
    assert_eq!(b.add("NiTriShape", w.buf), 2);
    let mut w = W::new();
    let verts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    write_geometry_data(&mut w, &verts, &[], None, None, None);
    w.u16(1); // num triangles
    w.u32(3); // num triangle points
    w.bool(true); // has triangles
    w.triangle([0, 1, 2]);
    w.u16(1); // num match groups
    w.u16(2); // group size
    w.u16(0);
    w.u16(1);
    assert_eq!(b.add("NiTriShapeData", w.buf), 3);

    let bytes = b.finish(100);
    let nif = NifFile::from_bytes(&bytes).unwrap();
    let meshes = nif.meshes();
    assert_eq!(meshes.len(), 1);
    let panel = &meshes[0];
    assert_eq!(panel.name, "Panel");
    // Composed: parent t + parent scale * local t
    // = (48,4,88) + 2*(10,0,0) = (68,4,88); scale 2*1 = 2.
    assert_eq!(panel.transform.translation, [68.0, 4.0, 88.0]);
    assert_eq!(panel.transform.scale, 2.0);
    // Vertices are NOT baked — the transform carries the placement.
    assert_eq!(panel.vertices[1], [1.0, 0.0, 0.0]);
}

#[test]
fn bs_fade_node_parses_as_node() {
    // BSFadeNode is the standard root of most Skyrim SE meshes and
    // has the NiNode layout. It must parse as a node with children,
    // or every shape under it is orphaned from the tree walk.
    let mut b = Builder::new();
    let mut w = W::new();
    write_node(&mut w, &mut b, "Scene Root", [5.0, 0.0, 0.0], 1.0, &[1]);
    assert_eq!(b.add("BSFadeNode", w.buf), 0);
    let mut w = W::new();
    write_bs_tri_shape(&mut w, &mut b, "FadeShape", -1);
    assert_eq!(b.add("BSTriShape", w.buf), 1);

    let bytes = b.finish(100);
    let nif = NifFile::from_bytes(&bytes).unwrap();
    match nif.block(0) {
        Some(Block::Node(node)) => {
            assert_eq!(node.children, vec![1]);
            assert_eq!(node.av.translation, [5.0, 0.0, 0.0]);
        }
        other => panic!("BSFadeNode did not parse as a node: {other:?}"),
    }
    let meshes = nif.meshes();
    assert_eq!(meshes.len(), 1);
    // The shape inherits the fade node's +5 x translation.
    assert_eq!(meshes[0].transform.translation, [5.0, 0.0, 0.0]);
}

fn write_switch(
    w: &mut W,
    b: &mut Builder,
    name: &str,
    children: &[i32],
    flags: u16,
    active_child: i32,
) {
    write_node(w, b, name, [0.0, 0.0, 0.0], 1.0, children);
    w.u16(flags);
    w.i32(active_child);
}

/// Build the pine structure, reduced: a BSTreeNode root, a
/// NiSwitchNode with a full-detail child (block 2, shape
/// "FullShape") and a LOD child (block 4, shape "LodShape"),
/// and the switch's stored active-child index.
fn build_switch_nif(active_child: i32) -> Vec<u8> {
    let mut b = Builder::new();
    let mut w = W::new();
    write_node(&mut w, &mut b, "Tree", [0.0, 0.0, 0.0], 1.0, &[1]);
    assert_eq!(b.add("BSTreeNode", w.buf), 0);
    let mut w = W::new();
    write_switch(&mut w, &mut b, "Switch", &[2, 4], 3, active_child);
    assert_eq!(b.add("NiSwitchNode", w.buf), 1);
    let mut w = W::new();
    write_node(&mut w, &mut b, "Full", [0.0, 0.0, 0.0], 1.0, &[3]);
    assert_eq!(b.add("NiNode", w.buf), 2);
    let mut w = W::new();
    write_geometry(
        &mut w,
        &mut b,
        "FullShape",
        6,
        -1,
        -1,
        &[],
        -1,
        [0.0, 0.0, 0.0],
        1.0,
    );
    assert_eq!(b.add("NiTriShape", w.buf), 3);
    let mut w = W::new();
    write_node(&mut w, &mut b, "Lod", [0.0, 0.0, 0.0], 1.0, &[5]);
    assert_eq!(b.add("NiNode", w.buf), 4);
    let mut w = W::new();
    write_geometry(
        &mut w,
        &mut b,
        "LodShape",
        7,
        -1,
        -1,
        &[],
        -1,
        [0.0, 0.0, 0.0],
        1.0,
    );
    assert_eq!(b.add("NiTriShape", w.buf), 5);
    for _ in 0..2 {
        let mut w = W::new();
        let verts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        write_geometry_data(&mut w, &verts, &[], None, None, None);
        w.u16(1); // num triangles
        w.u32(3); // num triangle points
        w.bool(true); // has triangles
        w.triangle([0, 1, 2]);
        w.u16(1); // num match groups
        w.u16(2); // group size
        w.u16(0);
        w.u16(1);
        b.add("NiTriShapeData", w.buf);
    }
    b.finish(100)
}

#[test]
fn bs_multi_bound_node_parses_as_node() {
    // BSMultiBoundNode is a NiNode prefix plus one i32 ref to
    // its BSMultiBound. It sits between the BSTreeNode root and
    // the NiSwitchNode in every pine NIF — without it the walk
    // dies before the switch and both trees render.
    let mut b = Builder::new();
    let mut w = W::new();
    write_node(&mut w, &mut b, "Bounds", [0.0, 0.0, 0.0], 1.0, &[1]);
    w.i32(-1); // multi bound ref (none in this fixture)
    assert_eq!(b.add("BSMultiBoundNode", w.buf), 0);
    let mut w = W::new();
    write_bs_tri_shape(&mut w, &mut b, "BoundShape", -1);
    assert_eq!(b.add("BSTriShape", w.buf), 1);

    let bytes = b.finish(100);
    let nif = NifFile::from_bytes(&bytes).unwrap();
    match nif.block(0) {
        Some(Block::Node(node)) => {
            assert_eq!(node.children, vec![1]);
        }
        other => panic!("BSMultiBoundNode did not parse as a node: {other:?}"),
    }
    let meshes = nif.meshes();
    assert_eq!(meshes.len(), 1);
    assert_eq!(meshes[0].name, "BoundShape");
}

#[test]
fn bs_ordered_node_parses_as_node() {
    // BSOrderedNode is a NiNode with no extra fields — a pure
    // ordering marker. Effect NIFs (waterfalls) hang meshes
    // under it; as Unknown the walk died and the shapes fell
    // back to local transforms.
    let mut b = Builder::new();
    let mut w = W::new();
    write_node(&mut w, &mut b, "Order", [0.0, 0.0, 0.0], 1.0, &[1]);
    assert_eq!(b.add("BSOrderedNode", w.buf), 0);
    let mut w = W::new();
    write_bs_tri_shape(&mut w, &mut b, "OrderedShape", -1);
    assert_eq!(b.add("BSTriShape", w.buf), 1);

    let bytes = b.finish(100);
    let nif = NifFile::from_bytes(&bytes).unwrap();
    match nif.block(0) {
        Some(Block::Node(node)) => {
            assert_eq!(node.children, vec![1]);
        }
        other => panic!("BSOrderedNode did not parse as a node: {other:?}"),
    }
    let meshes = nif.meshes();
    assert_eq!(meshes.len(), 1);
    assert_eq!(meshes[0].name, "OrderedShape");
}

#[test]
fn bs_tree_node_parses_as_node() {
    // BSTreeNode is the standard root of tree NIFs: a NiNode
    // prefix plus a 44-byte tail. It must parse as a node, or
    // the tree walk dies at block 0 for every pine.
    let bytes = build_switch_nif(0);
    let nif = NifFile::from_bytes(&bytes).unwrap();
    match nif.block(0) {
        Some(Block::Node(node)) => {
            assert_eq!(node.children, vec![1]);
        }
        other => panic!("BSTreeNode did not parse as a node: {other:?}"),
    }
}

#[test]
fn switch_node_renders_only_the_active_child() {
    // M13: a NiSwitchNode's children are alternatives — only the
    // stored active child renders. With index 0 the full-detail
    // shape renders and the LOD shape must NOT appear, neither
    // via the walk NOR via the orphan fallback (the fallback is
    // what would resurrect the LOD tree and double the pines).
    let bytes = build_switch_nif(0);
    let nif = NifFile::from_bytes(&bytes).unwrap();
    match nif.block(1) {
        Some(Block::SwitchNode(switch)) => {
            assert_eq!(switch.node.children, vec![2, 4]);
            assert_eq!(switch.active_child, 0);
        }
        other => panic!("NiSwitchNode did not parse as a switch: {other:?}"),
    }
    let meshes = nif.meshes();
    assert_eq!(meshes.len(), 1);
    assert_eq!(meshes[0].name, "FullShape");

    // Index 1 selects the LOD child instead.
    let bytes = build_switch_nif(1);
    let nif = NifFile::from_bytes(&bytes).unwrap();
    let meshes = nif.meshes();
    assert_eq!(meshes.len(), 1);
    assert_eq!(meshes[0].name, "LodShape");

    // An out-of-range index falls back to child 0.
    let bytes = build_switch_nif(9);
    let nif = NifFile::from_bytes(&bytes).unwrap();
    let meshes = nif.meshes();
    assert_eq!(meshes.len(), 1);
    assert_eq!(meshes[0].name, "FullShape");
}

#[test]
fn alpha_property_decodes_cutoff() {
    // The SE fixture's TestShape carries the alpha property (block
    // 8: flags 4844 = 0x12ec, threshold 128) — the common foliage
    // setup. BsShape has no alpha property.
    let bytes = build_se_nif();
    let nif = NifFile::from_bytes(&bytes).unwrap();
    let meshes = nif.meshes();
    let test = mesh_by_name(&meshes, "TestShape");
    assert_eq!(test.alpha_cutoff, Some(128.0 / 255.0));
    let bs = mesh_by_name(&meshes, "BsShape");
    assert_eq!(bs.alpha_cutoff, None);

    // Pure decode: test-enabled flags give a cutoff, blend-only
    // flags (bit 0 without bit 9) do not.
    assert_eq!(alpha_test_cutoff(0x12ec, 128), Some(128.0 / 255.0));
    assert_eq!(alpha_test_cutoff(0x1200, 200), Some(200.0 / 255.0));
    assert_eq!(alpha_test_cutoff(0x10ed, 128), None);
    assert_eq!(alpha_test_cutoff(0x100d, 128), None);
    assert_eq!(alpha_test_cutoff(0x0000, 128), None);
}

/// A `BSEffectShaderProperty` block in the real fog layout (see
/// the parser docs): net (no shader-type prefix), flags, UVs,
/// source texture, clamp, 4 floats, emissive Color4, 2 floats,
/// then the optional gradient string.
fn write_effect_shader(
    w: &mut W,
    b: &mut Builder,
    name: &str,
    source: &str,
    gradient: Option<&str>,
) {
    write_net(w, b, name, None);
    w.u32(0xe000_0078); // shader flags 1
    w.u32(0x4000_0030); // shader flags 2
    w.vec2([0.0, 0.0]); // UV offset
    w.vec2([1.0, 1.0]); // UV scale
    w.sized_string(source);
    w.u32(0x0000_a503); // texture clamp mode
    for v in [0.0, 0.0, 1.0, 0.0] {
        w.f32(v);
    }
    w.vec4([0.775, 0.775, 0.775, 1.0]); // emissive
    w.f32(0.75);
    w.f32(50.0);
    if let Some(g) = gradient {
        w.sized_string(g);
    } else {
        w.f32(0.0); // trailing float, block [22] shape
    }
}

#[test]
fn effect_shader_shape_is_blend_and_unlit() {
    // A minimal fog-shaped NIF: root node -> NiTriShape with an
    // effect shader (source FXFogHeavy.dds) and a blend-only alpha
    // property (0x10ed). The assembled mesh must carry Blend, no
    // cutoff, unlit, and the effect source texture as diffuse.
    // The lighting-shader sibling with the same alpha flags must
    // be Blend but lit.
    use dovah_nif::AlphaBlend;

    let mut b = Builder::new();

    // 0: NiNode "Scene Root", children 1, 4.
    let mut w = W::new();
    write_av(&mut w, &mut b, "Scene Root", [0.0, 0.0, 0.0], 1.0);
    w.u32(2);
    w.i32(1);
    w.i32(4);
    w.u32(0); // num effects
    assert_eq!(b.add("NiNode", w.buf), 0);

    // 1: NiTriShape "FogShape", data 2, effect shader 3, alpha 6.
    let mut w = W::new();
    write_geometry(
        &mut w,
        &mut b,
        "FogShape",
        2,
        3,
        6,
        &["FogMaterial"],
        0,
        [0.0, 0.0, 0.0],
        1.0,
    );
    assert_eq!(b.add("NiTriShape", w.buf), 1);

    // 2: NiTriShapeData: one triangle.
    let mut w = W::new();
    let verts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let normals = [[0.0, 0.0, 1.0]; 3];
    let colors = [[1.0, 1.0, 1.0, 0.5]; 3];
    let uvs = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
    write_geometry_data(&mut w, &verts, &normals, None, Some(&colors), Some(&uvs));
    w.u16(1); // num triangles
    w.u32(3); // num triangle points
    w.bool(true); // has triangles
    w.triangle([0, 1, 2]);
    w.u16(0); // num match groups
    assert_eq!(b.add("NiTriShapeData", w.buf), 2);

    // 3: BSEffectShaderProperty "FogShader", gradient present.
    let mut w = W::new();
    write_effect_shader(
        &mut w,
        &mut b,
        "FogShader",
        "textures\\effects\\FXFogHeavy.dds",
        Some("textures\\effects\\gradients\\GradCobWeb.dds"),
    );
    assert_eq!(b.add("BSEffectShaderProperty", w.buf), 3);

    // 4: NiTriShape "GlassShape", data 2, lighting shader 5, alpha 6.
    let mut w = W::new();
    write_geometry(
        &mut w,
        &mut b,
        "GlassShape",
        2,
        5,
        6,
        &["GlassMaterial"],
        0,
        [0.0, 0.0, 0.0],
        1.0,
    );
    assert_eq!(b.add("NiTriShape", w.buf), 4);

    // 5: BSLightingShaderProperty "GlassShader", type 0, no set.
    let mut w = W::new();
    write_net(&mut w, &mut b, "GlassShader", Some(0));
    w.u32(0x8240_0301);
    w.u32(0x8021);
    w.vec2([0.0, 0.0]);
    w.vec2([1.0, 1.0]);
    w.i32(-1); // no texture set
    w.vec3([0.0, 0.0, 0.0]);
    w.f32(1.0);
    w.u32(3);
    w.f32(1.0);
    w.f32(0.0);
    w.f32(80.0);
    w.vec3([1.0, 1.0, 1.0]);
    w.f32(1.0);
    w.f32(0.3);
    w.f32(2.0);
    assert_eq!(b.add("BSLightingShaderProperty", w.buf), 5);

    // 6: NiAlphaProperty, flags 0x10ed (normal blend).
    let mut w = W::new();
    write_net(&mut w, &mut b, "AlphaProp", None);
    w.u16(0x10ed);
    w.u8(0);
    assert_eq!(b.add("NiAlphaProperty", w.buf), 6);

    let bytes = b.finish(100);
    let nif = NifFile::from_bytes(&bytes).unwrap();

    // The effect shader block parsed (not Unknown).
    let Block::EffectShaderProperty(shader) = &nif.blocks[3] else {
        panic!("expected EffectShaderProperty at block 3");
    };
    assert_eq!(shader.source_texture, "textures\\effects\\FXFogHeavy.dds");
    assert_eq!(
        shader.gradient_texture.as_deref(),
        Some("textures\\effects\\gradients\\GradCobWeb.dds")
    );

    let meshes = nif.meshes();
    let fog = mesh_by_name(&meshes, "FogShape");
    assert_eq!(fog.alpha_blend, AlphaBlend::Blend);
    assert_eq!(fog.alpha_cutoff, None);
    assert!(fog.unlit);
    assert_eq!(
        fog.textures.diffuse.as_deref(),
        Some("textures\\effects\\FXFogHeavy.dds")
    );
    assert_eq!(
        fog.textures.gradient.as_deref(),
        Some("textures\\effects\\gradients\\GradCobWeb.dds")
    );

    let glass = mesh_by_name(&meshes, "GlassShape");
    assert_eq!(glass.alpha_blend, AlphaBlend::Blend);
    assert!(!glass.unlit);
    assert_eq!(glass.textures.diffuse, None);
    assert_eq!(glass.textures.gradient, None);
}

// ---------------------------------------------------------------------------
// Collision (bhk) round-trip
// ---------------------------------------------------------------------------

/// A 228-byte `bhkRigidBody` blob: shape ref, layer, collision
/// response (1 = colliding, 2 = non-colliding), fixed mass.
fn write_rigid_body(w: &mut W, shape: i32, layer: u8, response: u8) {
    w.i32(shape);
    w.u8(layer);
    w.u8(0); // filter flags
    w.u16(0); // group
    w.buf.extend_from_slice(&[0u8; 20]);
    w.u8(response);
    w.buf.extend_from_slice(&[0u8; 3]);
    w.f32(0.0); // mass
    w.u8(layer); // second filter layer
    w.u8(0);
    w.u16(0);
    w.f32(0.5); // friction
    w.f32(0.4); // restitution
    w.u8(response);
    w.buf.extend_from_slice(&[0u8; 7]);
    w.u8(5); // motion system
    w.u8(0); // quality
    w.buf.extend_from_slice(&[0u8; 170]);
}

/// A `bhkBoxShape` blob: 16 header bytes + half-extents (metres).
fn write_box_shape(w: &mut W, half_extents: [f32; 3]) {
    w.buf.extend_from_slice(&[0u8; 16]);
    w.vec3(half_extents);
}

#[test]
fn collision_shapes_box_hull_and_skips() {
    let mut b = Builder::new();

    // 0: NiNode "Scene Root" at (100, 0, 0) — the hull must be
    // offset by the node frame.
    let mut w = W::new();
    write_av(&mut w, &mut b, "Scene Root", [100.0, 0.0, 0.0], 1.0);
    w.u32(0); // num children
    w.u32(0); // num effects
    assert_eq!(b.add("NiNode", w.buf), 0);

    // 1: bhkCollisionObject -> node 0, body 2.
    let mut w = W::new();
    w.i32(0);
    w.u16(0x0081);
    w.i32(2);
    assert_eq!(b.add("bhkCollisionObject", w.buf), 1);

    // 2: bhkRigidBody, layer 1 STATIC, colliding, shape 3.
    let mut w = W::new();
    write_rigid_body(&mut w, 3, 1, 1);
    assert_eq!(w.buf.len(), 228);
    assert_eq!(b.add("bhkRigidBody", w.buf), 2);

    // 3: bhkBoxShape, half-extents (1, 2, 3) m.
    let mut w = W::new();
    write_box_shape(&mut w, [1.0, 2.0, 3.0]);
    assert_eq!(b.add("bhkBoxShape", w.buf), 3);

    // 4: bhkCollisionObject -> node 0, body 5 (layer 15:
    // NONCOLLIDABLE — must be skipped).
    let mut w = W::new();
    w.i32(0);
    w.u16(0x0081);
    w.i32(5);
    assert_eq!(b.add("bhkCollisionObject", w.buf), 4);
    let mut w = W::new();
    write_rigid_body(&mut w, 6, 15, 1);
    assert_eq!(b.add("bhkRigidBody", w.buf), 5);
    let mut w = W::new();
    write_box_shape(&mut w, [9.0, 9.0, 9.0]);
    assert_eq!(b.add("bhkBoxShape", w.buf), 6);

    // 7: bhkCollisionObject -> node 0, body 8 (response 2:
    // non-colliding — must be skipped).
    let mut w = W::new();
    w.i32(0);
    w.u16(0x0081);
    w.i32(8);
    assert_eq!(b.add("bhkCollisionObject", w.buf), 7);
    let mut w = W::new();
    write_rigid_body(&mut w, 9, 1, 2);
    assert_eq!(b.add("bhkRigidBody", w.buf), 8);
    let mut w = W::new();
    write_box_shape(&mut w, [9.0, 9.0, 9.0]);
    assert_eq!(b.add("bhkBoxShape", w.buf), 9);

    let bytes = b.finish(100);
    let nif = NifFile::from_bytes(&bytes).expect("parse synthetic collision NIF");
    let shapes = nif.collision_shapes();
    assert_eq!(shapes.len(), 1, "only the layer-1 colliding body survives");
    let CollisionShape::Hull { points, layer } = &shapes[0] else {
        panic!("expected a Hull, got {:?}", shapes[0]);
    };
    assert_eq!(*layer, 1);
    assert_eq!(points.len(), 8);
    // Half-extents (1,2,3) m = (70,140,210) u, offset by the
    // node's (100,0,0) translation.
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for p in points {
        for i in 0..3 {
            mn[i] = mn[i].min(p[i]);
            mx[i] = mx[i].max(p[i]);
        }
    }
    assert_eq!(mn, [30.0, -140.0, -210.0]);
    assert_eq!(mx, [170.0, 140.0, 210.0]);
}
