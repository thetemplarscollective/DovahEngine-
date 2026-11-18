//! Format conversion: dovah-nif meshes and dovah-dds textures into
//! Bevy assets.
//!
//! Mesh conversion applies the shape's transform in Skyrim space
//! first (rotation, uniform scale, translation), then converts
//! vertices and normals to Bevy space with [`crate::coords`].
//! The transform on a `dovah_nif::Mesh` is already the COMPOSED
//! scene-graph transform (parent nodes composed down to the shape,
//! M11) — conversion just applies it.
//!
//! Texture conversion decodes every mip level and packs them into a
//! single Bevy image with `mip_level_count` set, so mipmaps survive
//! into the GPU.

use crate::coords;
use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::math::{Vec3, Vec4};
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// v0.2.2: sanity cap on a single mesh's size. Real Skyrim meshes
/// top out far below this (the largest are ~100k vertices); an
/// absurd count means a corrupt or misread NIF, and allocating
/// for it would only OOM the phone. Capped meshes are logged and
/// skipped (the object falls back to a placeholder).
pub const MAX_MESH_VERTICES: usize = 1_000_000;
pub const MAX_MESH_INDICES: usize = 3_000_000;

/// Do a mesh's stored normals contradict its own winding?
/// FaceGen heads ship normals that point INTO the head (probe
/// K1: computed-vs-stored mean dot -0.40..-0.59 on every feature
/// shape) — the render trusts the winding and flips them.
/// Sampled (up to 64 triangles spread over the mesh) so the
/// check is cheap at scene build. The -0.3 threshold sits far
/// from both correct meshes (mean dot ~+1) and inverted ones
/// (~-0.5); mixed meshes (mean dot near 0 — e.g. a shape with
/// both inward and outward parts) never flip, and meshes with
/// fewer than 4 sampled triangles never flip either.
fn stored_normals_are_inverted(
    vertices: &[[f32; 3]],
    indices: &[u32],
    normals: &[[f32; 3]],
) -> bool {
    let tri_count = indices.len() / 3;
    if tri_count == 0 || normals.is_empty() {
        return false;
    }
    let stride = (tri_count / 64).max(1);
    let mut sum = 0.0f32;
    let mut n = 0usize;
    for t in (0..tri_count).step_by(stride) {
        let (ia, ib, ic) = (
            indices[t * 3] as usize,
            indices[t * 3 + 1] as usize,
            indices[t * 3 + 2] as usize,
        );
        if ia >= vertices.len()
            || ib >= vertices.len()
            || ic >= vertices.len()
            || ia >= normals.len()
            || ib >= normals.len()
            || ic >= normals.len()
        {
            continue;
        }
        let (a, b, c) = (vertices[ia], vertices[ib], vertices[ic]);
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let mut f = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let fl = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
        if fl < 1e-9 {
            continue;
        }
        f = [f[0] / fl, f[1] / fl, f[2] / fl];
        let mut s = [
            normals[ia][0] + normals[ib][0] + normals[ic][0],
            normals[ia][1] + normals[ib][1] + normals[ic][1],
            normals[ia][2] + normals[ib][2] + normals[ic][2],
        ];
        let sl = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
        if sl < 1e-6 {
            continue;
        }
        s = [s[0] / sl, s[1] / sl, s[2] / sl];
        sum += f[0] * s[0] + f[1] * s[1] + f[2] * s[2];
        n += 1;
    }
    n >= 4 && sum / n as f32 <= -0.3
}

/// Convert one dovah-nif mesh into a Bevy mesh. Returns `None` when
/// the mesh has no vertices or no indices.
pub fn nif_mesh_to_bevy(mesh: &dovah_nif::Mesh) -> Option<Mesh> {
    if mesh.vertices.is_empty() || mesh.indices.is_empty() {
        return None;
    }
    if mesh.vertices.len() > MAX_MESH_VERTICES || mesh.indices.len() > MAX_MESH_INDICES {
        crate::startup_log::log(&format!(
            "mesh '{}' skipped: {} vertices / {} indices exceeds the sanity cap",
            mesh.name,
            mesh.vertices.len(),
            mesh.indices.len()
        ));
        return None;
    }
    let rot = coords::nif_mat3_to_glam(mesh.transform.rotation);
    let scale = mesh.transform.scale;
    let trans = Vec3::new(
        mesh.transform.translation[0],
        mesh.transform.translation[1],
        mesh.transform.translation[2],
    );

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(mesh.vertices.len());
    for v in &mesh.vertices {
        let sky = rot * Vec3::new(v[0], v[1], v[2]) * scale + trans;
        positions.push(coords::to_bevy_position(sky.to_array()).to_array());
    }

    let mut bevy_mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    bevy_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    bevy_mesh.insert_indices(Indices::U32(mesh.indices.clone()));

    // v0.4.7: FaceGen heads ship stored normals that point INTO
    // the head (probe K1: computed-vs-stored mean dot -0.40..-0.59
    // on every feature shape of both inn NPCs) — the beard, brows,
    // eyes and mouth render ambient-only and the face reads as a
    // black void. The winding is correct (the skin, whose normals
    // Bevy computes from the winding, lights fine — the nose pixel
    // proves it), so when the stored normals contradict the
    // winding, trust the winding and flip them. The same factor
    // feeds the tangent handedness below, so normal mapping stays
    // consistent with the flipped normals.
    let normal_flip: f32 = if !mesh.normals.is_empty()
        && stored_normals_are_inverted(&mesh.vertices, &mesh.indices, &mesh.normals)
    {
        -1.0
    } else {
        1.0
    };
    if !mesh.normals.is_empty() {
        let normals: Vec<[f32; 3]> = mesh
            .normals
            .iter()
            .map(|n| {
                let rotated = rot * Vec3::new(n[0], n[1], n[2]);
                let b = coords::to_bevy_direction(rotated.to_array());
                [b.x * normal_flip, b.y * normal_flip, b.z * normal_flip]
            })
            .collect();
        bevy_mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    }
    if !mesh.uvs.is_empty() {
        bevy_mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, mesh.uvs.clone());
    }
    if !mesh.tangents.is_empty() && !mesh.bitangents.is_empty() && !mesh.normals.is_empty() {
        let tangents: Vec<[f32; 4]> = mesh
            .tangents
            .iter()
            .zip(mesh.bitangents.iter())
            .zip(mesh.normals.iter())
            .map(|((t, b), n)| {
                let tv = rot * Vec3::new(t[0], t[1], t[2]);
                let bv = rot * Vec3::new(b[0], b[1], b[2]);
                // v0.4.7: the handedness test uses the same
                // flipped normal the ATTRIBUTE_NORMAL insert
                // above wrote, so w stays consistent when the
                // stored normals were inverted.
                let nv = rot * Vec3::new(n[0], n[1], n[2]) * normal_flip;
                // Handedness: +1 when (tangent x bitangent) agrees
                // with the normal, -1 otherwise.
                let w = if tv.cross(bv).dot(nv) >= 0.0 {
                    1.0
                } else {
                    -1.0
                };
                let bt = coords::to_bevy_direction(tv.to_array());
                Vec4::new(bt.x, bt.y, bt.z, w).to_array()
            })
            .collect();
        bevy_mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
    }
    if !mesh.vertex_colors.is_empty() {
        // M13: for alpha-tested (cutout) shapes, the vertex alpha
        // must NOT feed the alpha test. Bevy PBR tests the
        // combined base-color alpha (texture x vertex), but the
        // full-detail pine foliage carries a low vertex alpha
        // (mean 0.43) meant for a blend pass we do not have — at
        // the 0.5 A2C cutoff (0.439 NIF threshold) most fragments
        // die and the tree renders as bare twigs. Bethesda's
        // cutout tests the texture alpha against the NIF
        // threshold; the vertex RGB still darkens the needles.
        // Force vertex alpha to 1.0 here so the test sees the
        // texture alpha alone. Non-cutout shapes keep their
        // vertex alpha untouched (nothing tests it).
        if mesh.alpha_cutoff.is_some() {
            let opaque: Vec<[f32; 4]> = mesh
                .vertex_colors
                .iter()
                .map(|c| [c[0], c[1], c[2], 1.0])
                .collect();
            bevy_mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, opaque);
        } else {
            bevy_mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, mesh.vertex_colors.clone());
        }
    }
    if mesh.normals.is_empty() {
        bevy_mesh.compute_smooth_normals();
    }
    Some(bevy_mesh)
}

/// Decode a DDS file's bytes into a Bevy image with mipmaps.
/// Returns `None` when the file is not a decodable DDS.
pub fn dds_to_bevy_image(bytes: &[u8]) -> Option<Image> {
    let dds = dovah_dds::DdsImage::from_bytes(bytes).ok()?;
    let mut data: Vec<u8> = Vec::new();
    let mut levels = 0u32;
    for level in 0..dds.mip_map_count {
        match dds.decode(level) {
            Ok(decoded) => {
                data.extend_from_slice(&decoded.data);
                levels += 1;
            }
            Err(_) => break,
        }
    }
    if levels == 0 {
        return None;
    }
    Some(image_from_rgba_mips(
        dds.width,
        dds.height,
        levels,
        data,
        TextureFormat::Rgba8UnormSrgb,
    ))
}

/// Decode a normal-map DDS (`*_n.dds`) into a Bevy image. Two
/// differences from the diffuse path, both load-bearing:
///
/// 1. The format is linear `Rgba8Unorm`, NOT sRGB — an sRGB
///    decode would bend every normal toward the flat direction.
/// 2. Z is reconstructed from X and Y per texel
///    (`z = sqrt(1 - x^2 - y^2)`). BC5 normal maps store only
///    X and Y (the decoder leaves B at 0, which would point
///    every normal sideways); BC3 maps store a Z that this
///    reproduces up to quantization, so reconstructing
///    unconditionally is correct for both formats.
///
/// The Y flip for DirectX-authored maps is NOT done here —
/// Bevy's `StandardMaterial::flip_normal_map_y` does it in the
/// shader (see `real_material`), which keeps the texture data
/// itself pristine.
pub fn dds_normal_to_bevy_image(bytes: &[u8]) -> Option<Image> {
    let dds = dovah_dds::DdsImage::from_bytes(bytes).ok()?;
    let mut data: Vec<u8> = Vec::new();
    let mut levels = 0u32;
    for level in 0..dds.mip_map_count {
        match dds.decode(level) {
            Ok(mut decoded) => {
                invert_normal_x(&mut decoded.data);
                reconstruct_normal_z(&mut decoded.data);
                data.extend_from_slice(&decoded.data);
                levels += 1;
            }
            Err(_) => break,
        }
    }
    if levels == 0 {
        return None;
    }
    Some(image_from_rgba_mips(
        dds.width,
        dds.height,
        levels,
        data,
        TextureFormat::Rgba8Unorm,
    ))
}

/// Invert the X (red) channel of an RGBA8 normal map in place:
/// Skyrim's normal maps encode +X opposite to the tangent frame
/// Bevy reconstructs from the NIF (T, w). Render-proven by
/// `floor_light_test.rs`: with R as-is, the StoneFloor01 mortar
/// joints read as RAISED ridges (bright wall on the far side of
/// the joint from the light); with R inverted they read as
/// RECESSED grooves (bright wall on the light side), matching
/// the green channel's DirectX flip (`flip_normal_map_y`). Both
/// channels are negated, i.e. the NIF tangent frame is 180
/// degrees from the baker's frame. Z is unaffected (x^2).
fn invert_normal_x(data: &mut [u8]) {
    for px in data.as_chunks_mut::<4>().0 {
        px[0] = 255 - px[0];
    }
}

/// Reconstruct the Z channel of an RGBA8 normal map in place:
/// each texel's (R, G) is (x, y) in [-1, 1], and Z is
/// `sqrt(max(0, 1 - x^2 - y^2))` — the unit-normal constraint.
/// BC5 maps arrive with B = 0 (no Z stored); BC3 maps arrive
/// with a stored Z this reproduces. Alpha is untouched.
fn reconstruct_normal_z(data: &mut [u8]) {
    for px in data.as_chunks_mut::<4>().0 {
        let x = px[0] as f32 / 255.0 * 2.0 - 1.0;
        let y = px[1] as f32 / 255.0 * 2.0 - 1.0;
        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
        px[2] = ((z + 1.0) / 2.0 * 255.0).round() as u8;
    }
}

/// Multiply two RGBA8 images channel-wise (c = a*b/255), sampling
/// the second image bilinearly with WRAP at each first-image texel.
/// The effect shader's gradient texture is smaller than the source
/// (64x64 vs 256x256) and tiles with the same UVs, so the gradient
/// must be resampled, not stretched 1:1. Pure, so the multiply is
/// unit-testable without a DDS in the room.
pub fn multiply_rgba8(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    grad: &[u8],
    grad_w: u32,
    grad_h: u32,
) -> Vec<u8> {
    let mut out = vec![0u8; src.len()];
    let gw = grad_w as f32;
    let gh = grad_h as f32;
    for y in 0..src_h {
        for x in 0..src_w {
            // Texel-centre UV, wrapped into [0,1).
            let u = ((x as f32 + 0.5) / src_w as f32).rem_euclid(1.0);
            let v = ((y as f32 + 0.5) / src_h as f32).rem_euclid(1.0);
            // Bilinear sample of the gradient at (u,v), wrapping.
            let fx = u * gw - 0.5;
            let fy = v * gh - 0.5;
            let x0 = fx.floor() as i32;
            let y0 = fy.floor() as i32;
            let tx = fx - x0 as f32;
            let ty = fy - y0 as f32;
            let sample = |ix: i32, iy: i32, c: usize| -> f32 {
                let gx = ix.rem_euclid(grad_w as i32) as usize;
                let gy = iy.rem_euclid(grad_h as i32) as usize;
                grad[(gy * grad_w as usize + gx) * 4 + c] as f32
            };
            let idx = ((y * src_w + x) * 4) as usize;
            for c in 0..4 {
                let g00 = sample(x0, y0, c);
                let g10 = sample(x0 + 1, y0, c);
                let g01 = sample(x0, y0 + 1, c);
                let g11 = sample(x0 + 1, y0 + 1, c);
                let g =
                    (g00 * (1.0 - tx) + g10 * tx) * (1.0 - ty) + (g01 * (1.0 - tx) + g11 * tx) * ty;
                let s = src[idx + c] as f32;
                out[idx + c] = ((s * g / 255.0).round() as u32).min(255) as u8;
            }
        }
    }
    out
}

/// Decode a source DDS multiplied by a gradient DDS into one Bevy
/// image — the `BSEffectShaderProperty` two-texture combine. The
/// game samples both textures with the same UVs and multiplies
/// them (RGB and alpha); the gradient is the effect's own
/// variation/falloff (GradCobWeb is a diagonal ramp, 78->194),
/// and without it the waterfall fog sheet reads as a flat slab.
/// Each mip level of the source is multiplied by the matching
/// gradient level (clamped to the gradient's smallest); a source
/// that fails to decode returns `None`, a gradient that fails
/// falls back to the plain source (the old behavior — a missing
/// gradient must not lose the fog).
pub fn dds_multiply_to_bevy_image(source: &[u8], gradient: &[u8]) -> Option<Image> {
    let src = dovah_dds::DdsImage::from_bytes(source).ok()?;
    let grad = match dovah_dds::DdsImage::from_bytes(gradient) {
        Ok(g) => g,
        // A gradient that fails to parse falls back to the plain
        // source — a missing gradient must not lose the fog.
        Err(_) => return dds_to_bevy_image(source),
    };
    let mut data: Vec<u8> = Vec::new();
    let mut levels = 0u32;
    for level in 0..src.mip_map_count {
        let Ok(decoded) = src.decode(level) else {
            break;
        };
        let grad_level = level.min(grad.mip_map_count.saturating_sub(1));
        let Ok(grad_decoded) = grad.decode(grad_level) else {
            return dds_to_bevy_image(source);
        };
        data.extend_from_slice(&multiply_rgba8(
            &decoded.data,
            decoded.width,
            decoded.height,
            &grad_decoded.data,
            grad_decoded.width,
            grad_decoded.height,
        ));
        levels += 1;
    }
    if levels == 0 {
        return None;
    }
    Some(image_from_rgba_mips(
        src.width,
        src.height,
        levels,
        data,
        TextureFormat::Rgba8UnormSrgb,
    ))
}

/// Build a Bevy image from concatenated RGBA8 mip levels (level 0
/// first, tightly packed). Shared by `dds_to_bevy_image` and
/// `dds_multiply_to_bevy_image` so the sampler rule lives in one
/// place.
fn image_from_rgba_mips(
    width: u32,
    height: u32,
    levels: u32,
    data: Vec<u8>,
    format: TextureFormat,
) -> Image {
    // Build via new_uninit: Image::new debug-asserts data.len() against a
    // single-mip extent, which fails for any DDS with mipmaps (data holds the
    // whole mip chain). new_uninit skips that check; the descriptor carries
    // the real mip count and the concatenated level data is exactly what
    // wgpu expects (level 0 first, each level tightly packed RGBA8).
    let mut image = Image::new_uninit(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = levels;
    image.data = Some(data);
    // M16 floor fix: Skyrim UVs tile — the inn floor's
    // StoneFloor01 runs U to 2.0, V from -3.0 to 1.0 (shape :16
    // of the FarmInt shell NIFs), and WoodPost02 tiles V too.
    // Bevy's default sampler is ClampToEdge, which smears the
    // edge pixel across any out-of-range UV into a flat plane
    // (the "ugly floor"). Repeat on all axes so tiled textures
    // tile. Every DDS world texture passes through this one
    // site; UI PNGs load through Bevy's own loader, not here,
    // so they are untouched.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        ..Default::default()
    });
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    use dovah_nif::{Mesh as NifMesh, Textures, Transform};

    fn triangle_mesh() -> NifMesh {
        NifMesh {
            name: "test".to_string(),
            vertices: vec![[0.0, 0.0, 0.0], [70.0, 0.0, 0.0], [0.0, 70.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            tangents: vec![],
            bitangents: vec![],
            vertex_colors: vec![],
            indices: vec![0, 1, 2],
            material_name: String::new(),
            textures: Textures::default(),
            alpha_cutoff: None,
            alpha_blend: dovah_nif::AlphaBlend::Opaque,
            unlit: false,
            emissive: [0.0, 0.0, 0.0],
            bounding_sphere: None,
            skinned: false,
            parent_node: String::new(),
            parent_transform: Transform::identity(),
            transform: Transform {
                translation: [0.0, 0.0, 0.0],
                rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                scale: 1.0,
            },
        }
    }

    #[test]
    fn triangle_converts_to_metres() {
        let bevy = nif_mesh_to_bevy(&triangle_mesh()).unwrap();
        let positions = bevy
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .unwrap()
            .as_float3()
            .unwrap();
        // (70, 0, 0) units -> (1, 0, 0) m; (0, 70, 0) -> (0, 0, -1) m.
        assert_eq!(positions[0], [0.0, 0.0, 0.0]);
        assert_eq!(positions[1], [1.0, 0.0, 0.0]);
        assert_eq!(positions[2], [0.0, 0.0, -1.0]);
        // Normal +Z (up) -> Bevy +Y.
        let normals = bevy
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .unwrap()
            .as_float3()
            .unwrap();
        assert_eq!(normals[0], [0.0, 1.0, 0.0]);
    }

    /// A flat 2x2 grid (3x3 verts, 8 triangles) in the XY plane,
    /// all wound CCW so the winding normal is +Z, with every
    /// stored normal set to `stored`.
    fn grid_mesh(stored: [f32; 3]) -> NifMesh {
        let mut mesh = triangle_mesh();
        let mut vertices = Vec::new();
        for y in 0..3 {
            for x in 0..3 {
                vertices.push([x as f32 * 70.0, y as f32 * 70.0, 0.0]);
            }
        }
        let mut indices = Vec::new();
        for j in 0..2 {
            for i in 0..2 {
                let a = (j * 3 + i) as u32;
                let b = a + 1;
                let c = a + 3;
                let d = c + 1;
                indices.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
        mesh.vertices = vertices;
        mesh.indices = indices;
        mesh.normals = vec![stored; 9];
        mesh.uvs = vec![[0.0, 0.0]; 9];
        mesh
    }

    fn output_normals(bevy: &Mesh) -> Vec<[f32; 3]> {
        bevy.attribute(Mesh::ATTRIBUTE_NORMAL)
            .unwrap()
            .as_float3()
            .unwrap()
            .to_vec()
    }

    #[test]
    fn inverted_stored_normals_flip_to_the_winding() {
        // v0.4.7 (probe K1): FaceGen feature shapes ship stored
        // normals pointing INTO the head — the beard/brows/eyes
        // render ambient-only and the face reads as a black void.
        // Stored (0,0,-1) against a +Z winding flips to +Z
        // (Bevy +Y) in the output.
        let bevy = nif_mesh_to_bevy(&grid_mesh([0.0, 0.0, -1.0])).unwrap();
        for n in output_normals(&bevy) {
            assert_eq!(n, [0.0, 1.0, 0.0]);
        }
    }

    #[test]
    fn correct_stored_normals_are_kept() {
        // The control: stored normals agreeing with the winding
        // pass through untouched (Skyrim +Z -> Bevy +Y).
        let bevy = nif_mesh_to_bevy(&grid_mesh([0.0, 0.0, 1.0])).unwrap();
        for n in output_normals(&bevy) {
            assert_eq!(n, [0.0, 1.0, 0.0]);
        }
    }

    #[test]
    fn cutout_mesh_forces_vertex_alpha_opaque() {
        // M13: the full pine foliage carries vertex alpha ~0.43
        // (a blend-path signal); fed into Bevy's alpha test it
        // kills most fragments and the tree renders as twigs.
        // Cutout shapes keep vertex RGB but force alpha to 1.0
        // so the test sees the texture alpha alone.
        let mut mesh = triangle_mesh();
        mesh.alpha_cutoff = Some(0.439);
        mesh.vertex_colors = vec![[0.8, 0.8, 0.8, 0.43]; 3];
        let bevy = nif_mesh_to_bevy(&mesh).unwrap();
        // Bevy 0.20 has `as_float3` but no `as_float4` — match
        // the variant directly.
        let bevy::mesh::VertexAttributeValues::Float32x4(colors) =
            bevy.attribute(Mesh::ATTRIBUTE_COLOR).unwrap()
        else {
            panic!("color attribute is not Float32x4");
        };
        assert_eq!(colors[0], [0.8, 0.8, 0.8, 1.0]);
        assert_eq!(colors[1], [0.8, 0.8, 0.8, 1.0]);
        assert_eq!(colors[2], [0.8, 0.8, 0.8, 1.0]);
        // Without a cutoff the vertex alpha passes through
        // untouched (nothing tests it).
        let mut mesh = triangle_mesh();
        mesh.vertex_colors = vec![[0.8, 0.8, 0.8, 0.43]; 3];
        let bevy = nif_mesh_to_bevy(&mesh).unwrap();
        let bevy::mesh::VertexAttributeValues::Float32x4(colors) =
            bevy.attribute(Mesh::ATTRIBUTE_COLOR).unwrap()
        else {
            panic!("color attribute is not Float32x4");
        };
        assert_eq!(colors[0], [0.8, 0.8, 0.8, 0.43]);
    }

    #[test]
    fn blend_mesh_keeps_vertex_alpha() {
        // The fog complex (newFog, vertex alpha 0.19-0.76) is
        // blend-only: its vertex alpha IS the transparency. It
        // must reach the shader untouched — forcing it to 1.0
        // would render the fog opaque again, transparent pass
        // or not.
        let mut mesh = triangle_mesh();
        mesh.alpha_blend = dovah_nif::AlphaBlend::Blend;
        mesh.vertex_colors = vec![[1.0, 1.0, 1.0, 0.38]; 3];
        let bevy = nif_mesh_to_bevy(&mesh).unwrap();
        let bevy::mesh::VertexAttributeValues::Float32x4(colors) =
            bevy.attribute(Mesh::ATTRIBUTE_COLOR).unwrap()
        else {
            panic!("color attribute is not Float32x4");
        };
        assert_eq!(colors[0], [1.0, 1.0, 1.0, 0.38]);
    }

    #[test]
    fn local_transform_applies_before_conversion() {
        let mut mesh = triangle_mesh();
        // Translate 70 units up in Skyrim space.
        mesh.transform.translation = [0.0, 0.0, 70.0];
        let bevy = nif_mesh_to_bevy(&mesh).unwrap();
        let positions = bevy
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .unwrap()
            .as_float3()
            .unwrap();
        assert_eq!(positions[0], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn absurd_mesh_is_skipped_not_allocated() {
        // v0.2.2: a corrupt NIF claiming millions of vertices must
        // not OOM the phone — it is logged and skipped.
        let mut mesh = triangle_mesh();
        mesh.vertices = vec![[0.0, 0.0, 0.0]; MAX_MESH_VERTICES + 1];
        mesh.indices = vec![0; 3];
        assert!(nif_mesh_to_bevy(&mesh).is_none());
        let mut mesh = triangle_mesh();
        mesh.indices = vec![0; MAX_MESH_INDICES + 1];
        assert!(nif_mesh_to_bevy(&mesh).is_none());
    }

    #[test]
    fn empty_mesh_returns_none() {
        let mut mesh = triangle_mesh();
        mesh.vertices.clear();
        assert!(nif_mesh_to_bevy(&mesh).is_none());
    }

    #[test]
    fn garbage_dds_returns_none() {
        assert!(dds_to_bevy_image(b"not a dds file").is_none());
        assert!(dds_to_bevy_image(&[]).is_none());
    }

    /// Regression test: a DDS with mipmaps must convert without tripping
    /// Bevy's single-mip `Image::new` debug assert (M6.1 — first real
    /// texture load panicked with 4096 vs 5456 bytes on a 32x32 5-mip DDS).
    #[test]
    fn multi_mip_dds_converts_with_mip_count() {
        // Minimal uncompressed 4x4 RGBA DDS, 2 mip levels (64 + 16 bytes).
        let mut dds: Vec<u8> = Vec::new();
        dds.extend_from_slice(b"DDS ");
        let mut header = vec![0u32; 31];
        header[0] = 124; // dwSize
        header[1] = 0x21007; // DDSD_CAPS | HEIGHT | WIDTH | PIXELFORMAT | MIPMAPCOUNT
        header[2] = 4; // height
        header[3] = 4; // width
        header[4] = 16; // pitch
        header[6] = 2; // mipMapCount
        header[18] = 32; // pixel format size
        header[19] = 0x41; // DDPF_RGB | DDPF_ALPHAPIXELS
        header[21] = 32; // RGB bit count
        header[22] = 0x00FF0000; // R mask
        header[23] = 0x0000FF00; // G mask
        header[24] = 0x000000FF; // B mask
        header[25] = 0xFF000000; // A mask
        header[26] = 0x401000; // DDSCAPS_TEXTURE | DDSCAPS_MIPMAP
        for word in header {
            dds.extend_from_slice(&word.to_le_bytes());
        }
        dds.extend_from_slice(&[255u8; 80]); // level 0 (64) + level 1 (16)
        let image = dds_to_bevy_image(&dds).expect("multi-mip DDS converts");
        assert_eq!(image.texture_descriptor.mip_level_count, 2);
        assert_eq!(image.texture_descriptor.size.width, 4);
        assert_eq!(image.texture_descriptor.size.height, 4);
        assert_eq!(image.data.as_ref().unwrap().len(), 80);
    }

    #[test]
    fn dds_world_textures_repeat_on_all_axes() {
        // M16 floor fix: the inn floor's UVs tile (U to 2.0, V
        // -3.0..1.0). A ClampToEdge sampler smears the edge pixel
        // across the whole floor into a flat tan plane; Repeat
        // lets the stone flags tile. Minimal uncompressed 4x4
        // RGBA DDS, 1 mip level — same construction as the
        // multi-mip test above.
        let mut dds: Vec<u8> = Vec::new();
        dds.extend_from_slice(b"DDS ");
        let mut header = vec![0u32; 31];
        header[0] = 124; // dwSize
        header[1] = 0x1007; // DDSD_CAPS | HEIGHT | WIDTH | PIXELFORMAT
        header[2] = 4; // height
        header[3] = 4; // width
        header[4] = 16; // pitch
        header[6] = 1; // mipMapCount
        header[18] = 32; // pixel format size
        header[19] = 0x41; // DDPF_RGB | DDPF_ALPHAPIXELS
        header[21] = 32; // RGB bit count
        header[22] = 0x00FF0000; // R mask
        header[23] = 0x0000FF00; // G mask
        header[24] = 0x000000FF; // B mask
        header[25] = 0xFF000000; // A mask
        header[26] = 0x1000; // DDSCAPS_TEXTURE
        for word in header {
            dds.extend_from_slice(&word.to_le_bytes());
        }
        dds.extend_from_slice(&[255u8; 64]); // level 0
        let image = dds_to_bevy_image(&dds).expect("DDS converts");
        let ImageSampler::Descriptor(desc) = &image.sampler else {
            panic!("world texture sampler must be an explicit descriptor, not Default");
        };
        assert_eq!(desc.address_mode_u, ImageAddressMode::Repeat);
        assert_eq!(desc.address_mode_v, ImageAddressMode::Repeat);
        assert_eq!(desc.address_mode_w, ImageAddressMode::Repeat);
    }

    #[test]
    fn multiply_rgba8_blends_and_wraps() {
        // 2x1 source, 2x1 gradient: texel centres land exactly
        // on gradient texels (u=0.25 -> grad 0, u=0.75 -> grad 1).
        let src = [255u8, 0, 0, 255, 0, 255, 0, 128];
        let grad = [128u8, 128, 128, 128, 255, 255, 255, 255];
        let out = multiply_rgba8(&src, 2, 1, &grad, 2, 1);
        assert_eq!(&out[0..4], &[128, 0, 0, 128]);
        assert_eq!(&out[4..8], &[0, 255, 0, 128]);
        // 1x1 source over the 2x1 gradient: the single texel
        // centre (u=0.5) bilinearly averages both gradient
        // texels — (128+255)/2 = 191.5 -> 192.
        let out = multiply_rgba8(&[255, 255, 255, 255], 1, 1, &grad, 2, 1);
        assert_eq!(&out[0..4], &[192, 192, 192, 192]);
        // Wrap: a 4x1 source's last texel (u=0.875) samples
        // between grad texel 1 and grad texel 0 (wrapped) —
        // 75% x 255 + 25% x 128 = 223, not a clamp to 255. The
        // first texel (u=0.125) wraps the other way: 25% x 255
        // + 75% x 128 = 160.
        let src = [255u8; 16];
        let out = multiply_rgba8(&src, 4, 1, &grad, 2, 1);
        assert_eq!(&out[12..16], &[223, 223, 223, 223]);
        assert_eq!(&out[0..4], &[160, 160, 160, 160]);
    }

    /// Minimal uncompressed RGBA DDS, 1 mip level, `pixels` in
    /// row-major RGBA order. Same header construction as the
    /// multi-mip test above.
    fn make_rgba_dds(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut dds: Vec<u8> = Vec::new();
        dds.extend_from_slice(b"DDS ");
        let mut header = vec![0u32; 31];
        header[0] = 124;
        header[1] = 0x1007;
        header[2] = height;
        header[3] = width;
        header[4] = width * 4;
        header[6] = 1;
        header[18] = 32;
        header[19] = 0x41;
        header[21] = 32;
        header[22] = 0x00FF0000;
        header[23] = 0x0000FF00;
        header[24] = 0x000000FF;
        header[25] = 0xFF000000;
        header[26] = 0x1000;
        for word in header {
            dds.extend_from_slice(&word.to_le_bytes());
        }
        dds.extend_from_slice(pixels);
        dds
    }

    #[test]
    fn dds_multiply_combines_source_and_gradient() {
        // 2x2 white source x 2x2 gradient (one 128 texel, rest
        // 255): the 128 corner multiplies to 128, the rest stay
        // white. A garbage gradient falls back to the plain
        // source instead of losing the fog.
        let src = make_rgba_dds(2, 2, &[255u8; 16]);
        let mut grad_px = [255u8; 16];
        grad_px[0] = 128;
        grad_px[1] = 128;
        grad_px[2] = 128;
        grad_px[3] = 128;
        let grad = make_rgba_dds(2, 2, &grad_px);
        let image = dds_multiply_to_bevy_image(&src, &grad).expect("multiply converts");
        let data = image.data.as_ref().unwrap();
        assert_eq!(&data[0..4], &[128, 128, 128, 128]);
        assert_eq!(&data[4..8], &[255, 255, 255, 255]);
        // Garbage gradient: plain source, white throughout.
        let image = dds_multiply_to_bevy_image(&src, b"not a dds file").expect("fallback converts");
        let data = image.data.as_ref().unwrap();
        assert!(data.iter().all(|&b| b == 255));
        // Garbage source: None either way.
        assert!(dds_multiply_to_bevy_image(b"not a dds file", &grad).is_none());
    }

    #[test]
    fn dds_normal_reconstructs_z_and_stays_linear() {
        // 2x1 normal map: texel 0 is (x=1, y=0) with B=0 (a
        // BC5-style texel — no Z stored); texel 1 is flat
        // (x=0, y=0) with B=0. Z reconstructs to 0 and 255,
        // and the image format is linear Unorm, not sRGB.
        // NOTE: make_rgba_dds writes BGRA-order pixels (its
        // masks put R at 0x00FF0000), so the input bytes are
        // (B, G, R, A) — the decoder swaps back to RGBA.
        let px = [
            0u8, 128, 255, 255, // RGBA (255,128,0) -> z=0 -> B=128
            0, 128, 128, 255, // RGBA (128,128,0) -> z=1 -> B=255
        ];
        let dds = make_rgba_dds(2, 1, &px);
        let image = dds_normal_to_bevy_image(&dds).expect("normal converts");
        assert_eq!(image.texture_descriptor.format, TextureFormat::Rgba8Unorm);
        let data = image.data.as_ref().unwrap();
        assert_eq!(data[2], 128);
        assert_eq!(data[6], 255);
        // R is INVERTED (render-proven — see invert_normal_x);
        // G passes through untouched (Bevy's flip_normal_map_y
        // handles the DirectX green convention).
        assert_eq!(data[0], 0);
        assert_eq!(data[4], 127);
        assert_eq!(data[1], 128);
        assert!(dds_normal_to_bevy_image(b"not a dds file").is_none());
    }
}
