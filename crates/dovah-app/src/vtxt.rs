//! VTXT terrain texture baking (M11 item 5).
//!
//! The ground's color comes from VTXT layers, not VCLR (which is
//! only a near-white tint). Each LAND cell carries a BTXT base
//! texture plus per-quadrant VTXT layers with per-vertex opacity.
//! This module bakes one cell's layers into a single sRGB texture
//! that the terrain mesh samples through its UVs (0..1 across the
//! cell), so the standard material pipeline renders dirt, grass,
//! rock and snow with no custom shader. The VCLR vertex colors
//! still multiply on top, exactly as the real game tints its
//! terrain textures.
//!
//! Bake model, per output pixel:
//! - cell vertex coords (vx, vy) in 0..32 from the pixel position
//!   (pixel row 0 = the south edge, matching UV v = y / 32);
//! - the owning quadrant from the quad (quadrants are 16x16 quads
//!   and never straddle a quad border);
//! - base color = the BTXT texture, tiled;
//! - for each layer of that quadrant in ascending order, blend
//!   toward the layer's (tiled) texture by the layer's opacity,
//!   bilinearly sampled from its 17x17 grid.
//!
//! Tiling scale is the one open parameter: one texture tile per
//! 4 quads (512 Skyrim units = 7.3 m), 8 tiles per cell. Tune by
//! eye against the real game's ground density.

use crate::assets::AssetProvider;
use crate::land::{LandData, QUADRANT_SIZE};
use bevy::image::Image;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;
use std::sync::Arc;

/// Baked terrain texture size (pixels per side). 9 cells x 4 MB.
pub const BAKE_SIZE: usize = 1024;

/// Texture tiling: one texture tile per N quads (vertex units).
/// 4 quads = 512 Skyrim units = 7.3 m per tile; 8 tiles per cell.
pub const TILE_QUADS: f32 = 4.0;

/// A decoded terrain texture (RGBA8, row-major), ready to sample.
pub struct DecodedTexture {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl DecodedTexture {
    /// Sample with wrapping (tiling) + bilinear filtering. `u` and
    /// `v` are in tile units and may exceed 1.0. Returns linear
    /// sRGB bytes as f32 (0.0-1.0) — the bake works in sRGB and
    /// the output image is marked sRGB, so no conversion here.
    pub fn sample(&self, u: f32, v: f32) -> [f32; 3] {
        let w = self.width as f32;
        let h = self.height as f32;
        // Texel-space coords, pixel centres at (i + 0.5).
        let fx = u.rem_euclid(1.0) * w - 0.5;
        let fy = v.rem_euclid(1.0) * h - 0.5;
        let x0 = fx.floor() as i32;
        let y0 = fy.floor() as i32;
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let w_i = self.width as i32;
        let h_i = self.height as i32;
        let px = |x: i32, y: i32| -> [f32; 3] {
            let xi = x.rem_euclid(w_i) as usize;
            let yi = y.rem_euclid(h_i) as usize;
            let i = (yi * self.width + xi) * 4;
            [
                self.data[i] as f32 / 255.0,
                self.data[i + 1] as f32 / 255.0,
                self.data[i + 2] as f32 / 255.0,
            ]
        };
        let c00 = px(x0, y0);
        let c10 = px(x0 + 1, y0);
        let c01 = px(x0, y0 + 1);
        let c11 = px(x0 + 1, y0 + 1);
        [
            bilerp(c00[0], c10[0], c01[0], c11[0], tx, ty),
            bilerp(c00[1], c10[1], c01[1], c11[1], tx, ty),
            bilerp(c00[2], c10[2], c01[2], c11[2], tx, ty),
        ]
    }
}

fn bilerp(c00: f32, c10: f32, c01: f32, c11: f32, tx: f32, ty: f32) -> f32 {
    c00 * (1.0 - tx) * (1.0 - ty) + c10 * tx * (1.0 - ty) + c01 * (1.0 - tx) * ty + c11 * tx * ty
}

/// Bilinear sample of a quadrant's 17x17 opacity grid at local
/// vertex coords (0..16, bottom-left origin).
fn sample_opacity(opacity: &[f32; QUADRANT_SIZE * QUADRANT_SIZE], lx: f32, ly: f32) -> f32 {
    let gx = lx.clamp(0.0, 16.0);
    let gy = ly.clamp(0.0, 16.0);
    let x0 = (gx.floor() as usize).min(15);
    let y0 = (gy.floor() as usize).min(15);
    let tx = gx - x0 as f32;
    let ty = gy - y0 as f32;
    bilerp(
        opacity[y0 * QUADRANT_SIZE + x0],
        opacity[y0 * QUADRANT_SIZE + x0 + 1],
        opacity[(y0 + 1) * QUADRANT_SIZE + x0],
        opacity[(y0 + 1) * QUADRANT_SIZE + x0 + 1],
        tx,
        ty,
    )
}

/// One quadrant's resolved VTXT layers, in blend order:
/// (layer index, texture, opacity grid).
type QuadrantLayers = Vec<(
    u16,
    Arc<DecodedTexture>,
    [f32; QUADRANT_SIZE * QUADRANT_SIZE],
)>;
/// caller-owned cache (the 9 Riverwood cells share ~9 textures).
/// A failed load/decodes caches as `None` so it is not retried.
/// `Arc`, not `Rc`: the cache lives in the `SpawnProgress`
/// resource (the chunked build), and Bevy resources must be
/// `Send + Sync`.
fn texture_for(
    path: &str,
    assets: &mut AssetProvider,
    cache: &mut HashMap<String, Option<Arc<DecodedTexture>>>,
) -> Option<Arc<DecodedTexture>> {
    if let Some(hit) = cache.get(path) {
        return hit.clone();
    }
    let decoded = assets.read_file(path).and_then(|bytes| {
        dovah_dds::DdsImage::from_bytes(&bytes)
            .ok()
            .and_then(|dds| dds.decode(0).ok())
            .map(|img| {
                Arc::new(DecodedTexture {
                    width: img.width as usize,
                    height: img.height as usize,
                    data: img.data,
                })
            })
    });
    if decoded.is_none() {
        crate::startup_log::log(&format!("VTXT bake: texture not decoded: {path}"));
    }
    cache.insert(path.to_string(), decoded.clone());
    decoded
}

/// Bake one cell's terrain texture. Returns `None` when the cell
/// has no resolvable base texture (the caller falls back to the
/// plain VCLR material).
pub fn bake_terrain_texture(
    land: &LandData,
    assets: &mut AssetProvider,
    cache: &mut HashMap<String, Option<Arc<DecodedTexture>>>,
) -> Option<Image> {
    let base_path = land.base_diffuse.as_ref()?;
    let base = texture_for(base_path, assets, cache)?;
    // Resolve the layers' textures up front, grouped by quadrant
    // and sorted ascending by layer (the blend order).
    let mut by_quadrant: [QuadrantLayers; 4] = Default::default();
    for layer in &land.layers {
        let Some(diffuse) = layer.diffuse.as_ref() else {
            continue;
        };
        let Some(tex) = texture_for(diffuse, assets, cache) else {
            continue;
        };
        let q = (layer.quadrant & 3) as usize;
        by_quadrant[q].push((layer.layer, tex, layer.opacity));
    }
    for layers in &mut by_quadrant {
        layers.sort_by_key(|(layer, _, _)| *layer);
    }

    let mut data = vec![0u8; BAKE_SIZE * BAKE_SIZE * 4];
    for py in 0..BAKE_SIZE {
        // Pixel row 0 = the south edge (v = 0 in the mesh UVs).
        let vy = (py as f32 + 0.5) / BAKE_SIZE as f32 * 32.0;
        let qy = (vy.floor() as usize).min(31);
        let qrow = if qy >= 16 { 2 } else { 0 };
        let lvy = vy - if qy >= 16 { 16.0 } else { 0.0 };
        for px in 0..BAKE_SIZE {
            let vx = (px as f32 + 0.5) / BAKE_SIZE as f32 * 32.0;
            let qx = (vx.floor() as usize).min(31);
            let quadrant = qrow + if qx >= 16 { 1 } else { 0 };
            let lvx = vx - if qx >= 16 { 16.0 } else { 0.0 };
            // Tiled texture coords (in tile units).
            let tu = vx / TILE_QUADS;
            let tv = vy / TILE_QUADS;
            let mut rgb = base.sample(tu, tv);
            for (_, tex, opacity) in &by_quadrant[quadrant] {
                let op = sample_opacity(opacity, lvx, lvy);
                if op <= 0.0 {
                    continue;
                }
                let layer_rgb = tex.sample(tu, tv);
                let op = op.clamp(0.0, 1.0);
                rgb = [
                    rgb[0] * (1.0 - op) + layer_rgb[0] * op,
                    rgb[1] * (1.0 - op) + layer_rgb[1] * op,
                    rgb[2] * (1.0 - op) + layer_rgb[2] * op,
                ];
            }
            let i = (py * BAKE_SIZE + px) * 4;
            data[i] = (rgb[0].clamp(0.0, 1.0) * 255.0) as u8;
            data[i + 1] = (rgb[1].clamp(0.0, 1.0) * 255.0) as u8;
            data[i + 2] = (rgb[2].clamp(0.0, 1.0) * 255.0) as u8;
            data[i + 3] = 255;
        }
    }
    Some(Image::new(
        Extent3d {
            width: BAKE_SIZE as u32,
            height: BAKE_SIZE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_wraps_and_interpolates() {
        // 2x2: red, green / blue, white.
        let tex = DecodedTexture {
            width: 2,
            height: 2,
            data: vec![
                255, 0, 0, 255, 0, 255, 0, 255, //
                0, 0, 255, 255, 255, 255, 255, 255,
            ],
        };
        // Centre of texel (0,0) is u=0.25, v=0.25 -> pure red.
        let c = tex.sample(0.25, 0.25);
        assert!(
            (c[0] - 1.0).abs() < 0.01 && c[1] < 0.01 && c[2] < 0.01,
            "{c:?}"
        );
        // Wrapping: u=1.25 == u=0.25.
        let c2 = tex.sample(1.25, 0.25);
        assert!((c2[0] - 1.0).abs() < 0.01, "{c2:?}");
        // Middle of all four texels (u=0.5, v=0.5) -> even mix.
        let m = tex.sample(0.5, 0.5);
        assert!((m[0] - 0.5).abs() < 0.01, "{m:?}");
        assert!((m[1] - 0.5).abs() < 0.01, "{m:?}");
        assert!((m[2] - 0.5).abs() < 0.01, "{m:?}");
    }

    #[test]
    fn sample_opacity_bilinear() {
        let mut grid = [0.0f32; QUADRANT_SIZE * QUADRANT_SIZE];
        grid[0] = 1.0; // (0,0)
        assert_eq!(sample_opacity(&grid, 0.0, 0.0), 1.0);
        assert_eq!(sample_opacity(&grid, 1.0, 0.0), 0.0);
        assert!((sample_opacity(&grid, 0.5, 0.0) - 0.5).abs() < 1e-6);
        assert!((sample_opacity(&grid, 0.5, 0.5) - 0.25).abs() < 1e-6);
        // Clamped at the far edge.
        assert_eq!(sample_opacity(&grid, 16.0, 16.0), 0.0);
    }

    #[test]
    fn quadrant_routing_by_pixel() {
        // Quadrant routing: q0 = SW = vx<16, vy<16 = pixels
        // px<512, py<512 (row 0 = south).
        let q = |px: usize, py: usize| -> usize {
            let vx = (px as f32 + 0.5) / BAKE_SIZE as f32 * 32.0;
            let vy = (py as f32 + 0.5) / BAKE_SIZE as f32 * 32.0;
            let qx = (vx.floor() as usize).min(31);
            let qy = (vy.floor() as usize).min(31);
            (if qy >= 16 { 2 } else { 0 }) + if qx >= 16 { 1 } else { 0 }
        };
        assert_eq!(q(0, 0), 0);
        assert_eq!(q(1023, 0), 1);
        assert_eq!(q(0, 1023), 2);
        assert_eq!(q(1023, 1023), 3);
        assert_eq!(q(511, 511), 0);
        assert_eq!(q(512, 511), 1);
        assert_eq!(q(511, 512), 2);
        assert_eq!(q(512, 512), 3);
    }
}
