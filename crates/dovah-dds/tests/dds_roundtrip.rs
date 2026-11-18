//! Round-trip tests for `dovah-dds`: build synthetic DDS files from
//! Microsoft's DDS documentation, parse them back, and check the
//! decoded pixels.

use dovah_dds::{DdsError, DdsImage, Format, RgbFormat};

// ---------------------------------------------------------------------------
// DDS builder
// ---------------------------------------------------------------------------

struct W {
    buf: Vec<u8>,
}

impl W {
    fn new() -> Self {
        W { buf: Vec::new() }
    }

    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
}

/// Pixel format description for the builder.
enum PixelFormat {
    /// FourCC (classic header).
    Fourcc([u8; 4]),
    /// Uncompressed: flags, bit count, R/G/B/A masks.
    Masks(u32, u32, u32, u32, u32, u32),
}

/// Build a DDS file: magic + header (+ DX10 header) + surface bytes.
fn build_dds(
    width: u32,
    height: u32,
    mip_count: u32,
    pixel_format: PixelFormat,
    dxgi_format: Option<u32>,
    surface: &[u8],
) -> Vec<u8> {
    let mut w = W::new();
    w.buf.extend_from_slice(b"DDS ");
    // DDS_HEADER (124 bytes).
    w.u32(124); // dwSize
    let mut flags = 0x1 | 0x2 | 0x4 | 0x1000; // CAPS | HEIGHT | WIDTH | PIXELFORMAT
    if mip_count > 1 {
        flags |= 0x20000; // MIPMAPCOUNT
    }
    w.u32(flags);
    w.u32(height);
    w.u32(width);
    w.u32(0); // pitch or linear size (unreliable; readers compute it)
    w.u32(0); // depth
    w.u32(mip_count);
    for _ in 0..11 {
        w.u32(0); // reserved1
    }
    // DDS_PIXELFORMAT (32 bytes).
    w.u32(32); // dwSize
    match pixel_format {
        PixelFormat::Fourcc(fourcc) => {
            w.u32(0x4); // DDPF_FOURCC
            w.buf.extend_from_slice(&fourcc);
            w.u32(0);
            w.u32(0);
            w.u32(0);
            w.u32(0);
            w.u32(0);
        }
        PixelFormat::Masks(pf_flags, bit_count, r, g, b, a) => {
            w.u32(pf_flags);
            w.u32(0); // no FourCC
            w.u32(bit_count);
            w.u32(r);
            w.u32(g);
            w.u32(b);
            w.u32(a);
        }
    }
    w.u32(0x1000); // caps: TEXTURE
    w.u32(0); // caps2
    w.u32(0); // caps3
    w.u32(0); // caps4
    w.u32(0); // reserved2
              // DDS_HEADER_DXT10 (20 bytes), when a DXGI format is given.
    if let Some(dxgi) = dxgi_format {
        w.u32(dxgi);
        w.u32(3); // resource dimension: 2D texture
        w.u32(0); // misc flag
        w.u32(1); // array size
        w.u32(0); // misc flags 2
    }
    w.buf.extend_from_slice(surface);
    w.buf
}

/// A BC1 block: two RGB565 endpoints + 32-bit indices.
fn bc1_block(color0: u16, color1: u16, indices: u32) -> [u8; 8] {
    let mut block = [0u8; 8];
    block[0..2].copy_from_slice(&color0.to_le_bytes());
    block[2..4].copy_from_slice(&color1.to_le_bytes());
    block[4..8].copy_from_slice(&indices.to_le_bytes());
    block
}

/// A BC3 block: alpha block (2 endpoints + 48-bit indices) + BC1.
fn bc3_block(alpha0: u8, alpha1: u8, alpha_indices: u64, color0: u16, color1: u16) -> [u8; 16] {
    let mut block = [0u8; 16];
    block[0] = alpha0;
    block[1] = alpha1;
    // 48-bit alpha indices, little-endian, in bytes 2..8.
    for i in 0..6 {
        block[2 + i] = ((alpha_indices >> (i * 8)) & 0xFF) as u8;
    }
    block[8..10].copy_from_slice(&color0.to_le_bytes());
    block[10..12].copy_from_slice(&color1.to_le_bytes());
    block
}

/// LSB-first bit writer for the hand-built BC7 block.
struct BitWriter {
    bytes: [u8; 16],
    pos: usize,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            bytes: [0u8; 16],
            pos: 0,
        }
    }

    fn write(&mut self, value: u32, bits: usize) {
        for i in 0..bits {
            if (value >> i) & 1 == 1 {
                self.bytes[self.pos / 8] |= 1 << (self.pos % 8);
            }
            self.pos += 1;
        }
    }
}

/// A BC7 mode-6 block (7-bit endpoints, 4-bit indices, one subset)
/// with every texel at endpoint 0: R0=127, G0=0, B0=0, A0=127.
/// Note: bcn_decode unquantizes 7-bit endpoints by left-shift, so
/// 127 decodes to 254, not 255.
fn bc7_mode6_red_block() -> [u8; 16] {
    let mut w = BitWriter::new();
    w.write(0x40, 7); // mode 6
    w.write(127, 7); // R0
    w.write(0, 7); // R1
    w.write(0, 7); // G0
    w.write(127, 7); // G1
    w.write(0, 7); // B0
    w.write(0, 7); // B1
    w.write(127, 7); // A0
    w.write(127, 7); // A1
    w.write(0, 3); // index 0 (anchor, 3 bits)
    for _ in 1..16 {
        w.write(0, 4); // indices 1..15
    }
    w.bytes
}

#[test]
fn decodes_uncompressed_rgba_with_mips() {
    // 4x4 RGBA8 (A8B8G8R8 masks), 3 mip levels: 4x4, 2x2, 1x1.
    let mut surface = Vec::new();
    // Mip 0: 16 pixels; pixel i = (i*16, 255 - i*16, 128, 255).
    for i in 0..16u8 {
        surface.extend_from_slice(&[i * 16, 255 - i * 16, 128, 255]);
    }
    // Mip 1: 4 pixels.
    for i in 0..4u8 {
        surface.extend_from_slice(&[10 + i, 20 + i, 30 + i, 40 + i]);
    }
    // Mip 2: 1 pixel.
    surface.extend_from_slice(&[200, 100, 50, 25]);

    let bytes = build_dds(
        4,
        4,
        3,
        PixelFormat::Masks(0x40 | 0x1, 32, 0xff, 0xff00, 0xff0000, 0xff000000),
        None,
        &surface,
    );
    let image = DdsImage::from_bytes(&bytes).unwrap();
    assert_eq!(image.width, 4);
    assert_eq!(image.height, 4);
    assert_eq!(image.mip_map_count, 3);
    assert!(!image.srgb);
    assert_eq!(
        image.format,
        Format::Rgb(RgbFormat {
            bits_per_pixel: 32,
            r_mask: 0xff,
            g_mask: 0xff00,
            b_mask: 0xff0000,
            a_mask: 0xff000000,
            luminance: false,
            alpha_only: false,
        })
    );
    assert_eq!(image.mip_dimensions(0), (4, 4));
    assert_eq!(image.mip_dimensions(1), (2, 2));
    assert_eq!(image.mip_dimensions(2), (1, 1));
    assert_eq!(image.mip_level_size(0), 64);
    assert_eq!(image.mip_level_size(1), 16);
    assert_eq!(image.mip_level_size(2), 4);
    assert_eq!(image.mip_offset(1), 64);
    assert_eq!(image.mip_offset(2), 80);

    let mip0 = image.decode(0).unwrap();
    assert_eq!(mip0.width, 4);
    assert_eq!(mip0.height, 4);
    assert_eq!(mip0.data.len(), 64);
    assert_eq!(&mip0.data[0..4], &[0, 255, 128, 255]);
    assert_eq!(&mip0.data[4..8], &[16, 239, 128, 255]);
    assert_eq!(&mip0.data[60..64], &[240, 15, 128, 255]);

    let mip1 = image.decode(1).unwrap();
    assert_eq!(mip1.width, 2);
    assert_eq!(mip1.height, 2);
    assert_eq!(&mip1.data[0..4], &[10, 20, 30, 40]);
    assert_eq!(&mip1.data[12..16], &[13, 23, 33, 43]);

    let mip2 = image.decode(2).unwrap();
    assert_eq!(mip2.width, 1);
    assert_eq!(mip2.height, 1);
    assert_eq!(mip2.data, vec![200, 100, 50, 25]);

    assert!(image.decode(3).is_err());
}

#[test]
fn decodes_uncompressed_24bit_bgr_masks() {
    // 2x2 R8G8B8 (24-bit, R mask 0xff0000): stored bytes are B, G, R.
    let surface: [u8; 12] = [
        0x00, 0x00, 0xFF, // red
        0x00, 0xFF, 0x00, // green
        0xFF, 0x00, 0x00, // blue
        0xFF, 0xFF, 0xFF, // white
    ];
    let bytes = build_dds(
        2,
        2,
        1,
        PixelFormat::Masks(0x40, 24, 0xff0000, 0xff00, 0xff, 0),
        None,
        &surface,
    );
    let image = DdsImage::from_bytes(&bytes).unwrap();
    let decoded = image.decode(0).unwrap();
    assert_eq!(decoded.width, 2);
    assert_eq!(decoded.height, 2);
    assert_eq!(&decoded.data[0..4], &[255, 0, 0, 255]);
    assert_eq!(&decoded.data[4..8], &[0, 255, 0, 255]);
    assert_eq!(&decoded.data[8..12], &[0, 0, 255, 255]);
    assert_eq!(&decoded.data[12..16], &[255, 255, 255, 255]);
}

#[test]
fn decodes_uncompressed_16bit_565() {
    // 2x1 R5G6B5: red (0xF800) and green (0x07E0).
    let surface: [u8; 4] = [0x00, 0xF8, 0xE0, 0x07];
    let bytes = build_dds(
        2,
        1,
        1,
        PixelFormat::Masks(0x40, 16, 0xf800, 0x7e0, 0x1f, 0),
        None,
        &surface,
    );
    let image = DdsImage::from_bytes(&bytes).unwrap();
    let decoded = image.decode(0).unwrap();
    assert_eq!(&decoded.data[0..4], &[255, 0, 0, 255]);
    assert_eq!(&decoded.data[4..8], &[0, 255, 0, 255]);
}

#[test]
fn decodes_luminance() {
    // 2x1 L8: two grey levels.
    let surface: [u8; 2] = [0, 200];
    let bytes = build_dds(
        2,
        1,
        1,
        PixelFormat::Masks(0x20000, 8, 0xff, 0, 0, 0),
        None,
        &surface,
    );
    let image = DdsImage::from_bytes(&bytes).unwrap();
    let decoded = image.decode(0).unwrap();
    assert_eq!(&decoded.data[0..4], &[0, 0, 0, 255]);
    assert_eq!(&decoded.data[4..8], &[200, 200, 200, 255]);
}

#[test]
fn decodes_bc1_opaque() {
    // One BC1 block: color0 = red, color1 = blue, all indices 0.
    let block = bc1_block(0xF800, 0x001F, 0);
    let bytes = build_dds(4, 4, 1, PixelFormat::Fourcc(*b"DXT1"), None, &block);
    let image = DdsImage::from_bytes(&bytes).unwrap();
    assert_eq!(image.format, Format::Bc1);
    assert_eq!(image.mip_level_size(0), 8);
    let decoded = image.decode(0).unwrap();
    assert_eq!(decoded.width, 4);
    assert_eq!(decoded.height, 4);
    for pixel in decoded.data.as_chunks::<4>().0 {
        assert_eq!(pixel, &[255, 0, 0, 255]);
    }
}

#[test]
fn decodes_bc1_one_bit_alpha() {
    // Three-color mode (color0 < color1): index 3 is transparent.
    // Texel 0 uses index 3; the rest use index 0 (blue).
    let block = bc1_block(0x001F, 0xF800, 0b11);
    let bytes = build_dds(4, 4, 1, PixelFormat::Fourcc(*b"DXT1"), None, &block);
    let image = DdsImage::from_bytes(&bytes).unwrap();
    let decoded = image.decode(0).unwrap();
    assert_eq!(&decoded.data[0..4], &[0, 0, 0, 0]);
    assert_eq!(&decoded.data[4..8], &[0, 0, 255, 255]);
}

#[test]
fn decodes_bc1_mips() {
    // 8x8 BC1 with 2 mips: mip 0 is 4 blocks (red), mip 1 is 1 block
    // (blue).
    let mut surface = Vec::new();
    for _ in 0..4 {
        surface.extend_from_slice(&bc1_block(0xF800, 0x001F, 0));
    }
    surface.extend_from_slice(&bc1_block(0x001F, 0xF800, 0));
    let bytes = build_dds(8, 8, 2, PixelFormat::Fourcc(*b"DXT1"), None, &surface);
    let image = DdsImage::from_bytes(&bytes).unwrap();
    assert_eq!(image.mip_level_size(0), 32);
    assert_eq!(image.mip_level_size(1), 8);
    assert_eq!(image.mip_offset(1), 32);
    let mip0 = image.decode(0).unwrap();
    assert_eq!(mip0.width, 8);
    for pixel in mip0.data.as_chunks::<4>().0 {
        assert_eq!(pixel, &[255, 0, 0, 255]);
    }
    let mip1 = image.decode(1).unwrap();
    assert_eq!(mip1.width, 4);
    for pixel in mip1.data.as_chunks::<4>().0 {
        assert_eq!(pixel, &[0, 0, 255, 255]);
    }
}

#[test]
fn decodes_bc3() {
    // One BC3 block: alpha endpoints 255/0, all alpha indices 0
    // (alpha 255); color red.
    let block = bc3_block(255, 0, 0, 0xF800, 0x001F);
    let bytes = build_dds(4, 4, 1, PixelFormat::Fourcc(*b"DXT5"), None, &block);
    let image = DdsImage::from_bytes(&bytes).unwrap();
    assert_eq!(image.format, Format::Bc3);
    assert_eq!(image.mip_level_size(0), 16);
    let decoded = image.decode(0).unwrap();
    for pixel in decoded.data.as_chunks::<4>().0 {
        assert_eq!(pixel, &[255, 0, 0, 255]);
    }
}

#[test]
fn decodes_bc7_via_dx10() {
    // BC7 through the DX10 header (dxgiFormat 98 = BC7_UNORM).
    let block = bc7_mode6_red_block();
    let bytes = build_dds(4, 4, 1, PixelFormat::Fourcc(*b"DX10"), Some(98), &block);
    let image = DdsImage::from_bytes(&bytes).unwrap();
    assert_eq!(image.format, Format::Bc7);
    assert_eq!(image.dxgi_format, Some(98));
    assert!(!image.srgb);
    assert_eq!(image.mip_level_size(0), 16);
    let decoded = image.decode(0).unwrap();
    // 7-bit endpoint 127 unquantizes to 254 (left-shift).
    for pixel in decoded.data.as_chunks::<4>().0 {
        assert_eq!(pixel, &[254, 0, 0, 254]);
    }
}

#[test]
fn decodes_bc7_srgb_via_dx10() {
    // dxgiFormat 99 = BC7_UNORM_SRGB: same bytes, srgb flag set.
    let block = bc7_mode6_red_block();
    let bytes = build_dds(4, 4, 1, PixelFormat::Fourcc(*b"DX10"), Some(99), &block);
    let image = DdsImage::from_bytes(&bytes).unwrap();
    assert_eq!(image.format, Format::Bc7);
    assert!(image.srgb);
    let decoded = image.decode(0).unwrap();
    assert_eq!(&decoded.data[0..4], &[254, 0, 0, 254]);
}

#[test]
fn decodes_dx10_rgba8() {
    // dxgiFormat 28 = R8G8B8A8_UNORM, 2x2.
    let surface: [u8; 16] = [
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
    ];
    let bytes = build_dds(2, 2, 1, PixelFormat::Fourcc(*b"DX10"), Some(28), &surface);
    let image = DdsImage::from_bytes(&bytes).unwrap();
    assert_eq!(
        image.format,
        Format::Rgb(RgbFormat {
            bits_per_pixel: 32,
            r_mask: 0xff,
            g_mask: 0xff00,
            b_mask: 0xff0000,
            a_mask: 0xff000000,
            luminance: false,
            alpha_only: false,
        })
    );
    let decoded = image.decode(0).unwrap();
    assert_eq!(decoded.data, surface.to_vec());
}

#[test]
fn opens_from_disk() {
    let block = bc1_block(0xF800, 0x001F, 0);
    let bytes = build_dds(4, 4, 1, PixelFormat::Fourcc(*b"DXT1"), None, &block);
    let path = std::env::temp_dir().join("dovah-dds-test.dds");
    std::fs::write(&path, &bytes).unwrap();
    let image = DdsImage::open(&path).unwrap();
    assert_eq!(image.format, Format::Bc1);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn rejects_bad_magic() {
    let err = DdsImage::from_bytes(&[0u8; 128]).unwrap_err();
    assert!(matches!(err, DdsError::NotADds));
}

#[test]
fn rejects_truncated() {
    let block = bc1_block(0xF800, 0x001F, 0);
    let bytes = build_dds(4, 4, 1, PixelFormat::Fourcc(*b"DXT1"), None, &block);
    assert!(DdsImage::from_bytes(&bytes[..64]).is_err());
    // Header present but surface data missing.
    assert!(DdsImage::from_bytes(&bytes[..128]).is_err());
    assert!(DdsImage::from_bytes(&[]).is_err());
}

#[test]
fn rejects_bad_header_size() {
    let block = bc1_block(0xF800, 0x001F, 0);
    let mut bytes = build_dds(4, 4, 1, PixelFormat::Fourcc(*b"DXT1"), None, &block);
    // dwSize is the first u32 after the magic.
    bytes[4..8].copy_from_slice(&100u32.to_le_bytes());
    let err = DdsImage::from_bytes(&bytes).unwrap_err();
    assert!(matches!(err, DdsError::InvalidData(_)));
}

#[test]
fn rejects_unsupported_fourcc() {
    let block = [0u8; 8];
    let bytes = build_dds(4, 4, 1, PixelFormat::Fourcc(*b"UYVY"), None, &block);
    let err = DdsImage::from_bytes(&bytes).unwrap_err();
    assert!(matches!(err, DdsError::UnsupportedFourcc(fourcc) if &fourcc == b"UYVY"));
}

#[test]
fn rejects_unsupported_dxgi_format() {
    let block = [0u8; 16];
    let bytes = build_dds(4, 4, 1, PixelFormat::Fourcc(*b"DX10"), Some(999), &block);
    let err = DdsImage::from_bytes(&bytes).unwrap_err();
    assert!(matches!(err, DdsError::UnsupportedDxgiFormat(999)));
}

#[test]
fn rejects_zero_dimensions() {
    let bytes = build_dds(
        0,
        4,
        1,
        PixelFormat::Masks(0x40 | 0x1, 32, 0xff, 0xff00, 0xff0000, 0xff000000),
        None,
        &[],
    );
    assert!(matches!(
        DdsImage::from_bytes(&bytes).unwrap_err(),
        DdsError::InvalidData(_)
    ));
}
