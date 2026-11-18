//! DDS texture reader for Dovahengine.
//!
//! Reads DirectDraw Surface (DDS) texture files — the format Skyrim
//! Special Edition stores its textures in — following Microsoft's
//! public DDS documentation (`docs/formats/dds.md`). This crate
//! contains no Bethesda code and ships no game data: it parses
//! texture files the user already owns.
//!
//! The reader parses the DDS header (and the DX10 extension header
//! when present), resolves the pixel format, and decodes mip levels
//! to raw RGBA8 bytes. Uncompressed RGB(A), luminance, and alpha-only
//! data are unpacked by hand from the bit masks; block-compressed
//! formats (BC1-BC7) decode through the [`bcn_decode`] crate — see
//! the notes file for the rationale.
//!
//! Decoding covers the first face/slice of the texture. Cube maps,
//! volume textures, and texture arrays expose their header facts
//! (`is_cube_map`, `is_volume`, `array_size`); decoding their
//! remaining faces/slices is future work for the renderer phase.

use std::fs;
use std::io;
use std::path::Path;

const DDS_MAGIC: &[u8; 4] = b"DDS ";
const DDS_HEADER_SIZE: u32 = 124;
const DDS_PIXELFORMAT_SIZE: u32 = 32;
const DDS_HEADER_DXT10_SIZE: usize = 20;

// DDS_PIXELFORMAT dwFlags (DDPF_*).
const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_ALPHA: u32 = 0x2;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_RGB: u32 = 0x40;
const DDPF_LUMINANCE: u32 = 0x20000;

// DDS_HEADER dwCaps2 (DDSCAPS2_*).
const DDSCAPS2_CUBEMAP: u32 = 0x200;
const DDSCAPS2_VOLUME: u32 = 0x200000;

// DDS_HEADER_DXT10 miscFlag.
const DDS_RESOURCE_MISC_TEXTURECUBE: u32 = 0x4;

#[derive(Debug)]
pub enum DdsError {
    Io(io::Error),
    /// The file does not start with the `DDS ` magic.
    NotADds,
    Truncated(&'static str),
    InvalidData(&'static str),
    /// A FourCC the reader does not know (e.g. a YUV format).
    UnsupportedFourcc([u8; 4]),
    /// A DXGI format value the reader does not know.
    UnsupportedDxgiFormat(u32),
    /// The block decoder rejected the surface data.
    Decode(&'static str),
}

impl std::fmt::Display for DdsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DdsError::Io(e) => write!(f, "I/O error: {e}"),
            DdsError::NotADds => write!(f, "not a DDS file (missing DDS magic)"),
            DdsError::Truncated(what) => write!(f, "truncated DDS in {what}"),
            DdsError::InvalidData(what) => write!(f, "invalid DDS data: {what}"),
            DdsError::UnsupportedFourcc(fourcc) => write!(
                f,
                "unsupported DDS FourCC '{}'",
                String::from_utf8_lossy(fourcc)
            ),
            DdsError::UnsupportedDxgiFormat(value) => {
                write!(f, "unsupported DXGI format {value}")
            }
            DdsError::Decode(what) => write!(f, "failed to decode DDS surface: {what}"),
        }
    }
}

impl std::error::Error for DdsError {}

impl From<io::Error> for DdsError {
    fn from(e: io::Error) -> Self {
        DdsError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, DdsError>;

/// An uncompressed pixel format, described by bit masks (as stored in
/// `DDS_PIXELFORMAT`, or derived from a DXGI format value).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbFormat {
    pub bits_per_pixel: u32,
    pub r_mask: u32,
    pub g_mask: u32,
    pub b_mask: u32,
    pub a_mask: u32,
    /// `DDPF_LUMINANCE`: the R mask is a single luminance channel,
    /// copied to R, G, and B.
    pub luminance: bool,
    /// `DDPF_ALPHA` only: an alpha channel with no color data; RGB
    /// decodes to white.
    pub alpha_only: bool,
}

/// The pixel format of a DDS file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Uncompressed pixels, described by bit masks.
    Rgb(RgbFormat),
    /// BC1 (DXT1), 8 bytes per 4x4 block. Decodes with 1-bit alpha
    /// semantics: a block in three-color mode yields transparent
    /// black for index 3, matching Direct3D.
    Bc1,
    /// BC2 (DXT2/DXT3), 16 bytes per block.
    Bc2,
    /// BC3 (DXT4/DXT5), 16 bytes per block.
    Bc3,
    /// BC4 (ATI1/BC4U), 8 bytes per block. Decodes to (R, 0, 0, 255).
    Bc4,
    /// BC4 SNORM (BC4S). Decodes to two's-complement SNORM bytes.
    Bc4Snorm,
    /// BC5 (ATI2/BC5U), 16 bytes per block. Decodes to (R, G, 0, 255).
    Bc5,
    /// BC5 SNORM (BC5S). Decodes to two's-complement SNORM bytes.
    Bc5Snorm,
    /// BC6H unsigned (UF16), 16 bytes per block. The 8-bit decode
    /// clamps to [0, 1]; HDR above 1.0 is lost.
    Bc6hUf16,
    /// BC6H signed (SF16), 16 bytes per block.
    Bc6hSf16,
    /// BC7, 16 bytes per block.
    Bc7,
}

impl Format {
    /// Whether this format is block-compressed.
    pub fn is_compressed(self) -> bool {
        !matches!(self, Format::Rgb(_))
    }

    /// Bytes per 4x4 block, for compressed formats.
    pub fn block_bytes(self) -> Option<usize> {
        match self {
            Format::Rgb(_) => None,
            Format::Bc1 | Format::Bc4 | Format::Bc4Snorm => Some(8),
            _ => Some(16),
        }
    }

    fn bcn_format(self) -> Option<bcn_decode::Format> {
        Some(match self {
            Format::Rgb(_) => return None,
            Format::Bc1 => bcn_decode::Format::Bc1Alpha,
            Format::Bc2 => bcn_decode::Format::Bc2,
            Format::Bc3 => bcn_decode::Format::Bc3,
            Format::Bc4 => bcn_decode::Format::Bc4,
            Format::Bc4Snorm => bcn_decode::Format::Bc4Snorm,
            Format::Bc5 => bcn_decode::Format::Bc5,
            Format::Bc5Snorm => bcn_decode::Format::Bc5Snorm,
            Format::Bc6hUf16 => bcn_decode::Format::Bc6hUf16,
            Format::Bc6hSf16 => bcn_decode::Format::Bc6hSf16,
            Format::Bc7 => bcn_decode::Format::Bc7,
        })
    }
}

/// A decoded image: RGBA8 bytes, row-major, `width * height * 4`
/// bytes long.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// A parsed DDS file: header info plus the surface data.
#[derive(Debug, Clone)]
pub struct DdsImage {
    pub width: u32,
    pub height: u32,
    /// Depth of a volume texture; 1 otherwise.
    pub depth: u32,
    pub format: Format,
    /// True for `_SRGB` DXGI formats. The decoded bytes are the same
    /// either way; sRGB is an interpretation for the renderer.
    pub srgb: bool,
    /// Number of mipmap levels (at least 1).
    pub mip_map_count: u32,
    /// Number of array elements (DX10 header); 1 for a plain texture.
    pub array_size: u32,
    pub is_cube_map: bool,
    pub is_volume: bool,
    /// Raw DXGI format value when a DX10 header is present.
    pub dxgi_format: Option<u32>,
    /// Raw FourCC from the pixel format.
    pub fourcc: [u8; 4],
    /// The surface data (all mip levels, faces, and slices), starting
    /// after the headers.
    data: Vec<u8>,
}

impl DdsImage {
    /// Parse a DDS file from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    /// Parse a DDS file from its bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 4 + DDS_HEADER_SIZE as usize {
            return Err(DdsError::Truncated("DDS header"));
        }
        if &bytes[0..4] != DDS_MAGIC {
            return Err(DdsError::NotADds);
        }
        let mut reader = Reader::new(&bytes[4..]);

        let dw_size = reader.u32("header size")?;
        if dw_size != DDS_HEADER_SIZE {
            return Err(DdsError::InvalidData("DDS header size"));
        }
        let _dw_flags = reader.u32("header flags")?;
        let height = reader.u32("height")?;
        let width = reader.u32("width")?;
        let _pitch_or_linear_size = reader.u32("pitch or linear size")?;
        let depth = reader.u32("depth")?;
        let mip_map_count_raw = reader.u32("mipmap count")?;
        for _ in 0..11 {
            let _reserved = reader.u32("reserved")?;
        }
        // DDS_PIXELFORMAT (32 bytes).
        let pf_size = reader.u32("pixel format size")?;
        if pf_size != DDS_PIXELFORMAT_SIZE {
            return Err(DdsError::InvalidData("pixel format size"));
        }
        let pf_flags = reader.u32("pixel format flags")?;
        let fourcc: [u8; 4] = reader.take(4, "FourCC")?.try_into().unwrap();
        let rgb_bit_count = reader.u32("RGB bit count")?;
        let r_mask = reader.u32("R bit mask")?;
        let g_mask = reader.u32("G bit mask")?;
        let b_mask = reader.u32("B bit mask")?;
        let a_mask = reader.u32("A bit mask")?;
        let _caps = reader.u32("caps")?;
        let caps2 = reader.u32("caps2")?;
        let _caps3 = reader.u32("caps3")?;
        let _caps4 = reader.u32("caps4")?;
        let _reserved2 = reader.u32("reserved2")?;

        if width == 0 || height == 0 {
            return Err(DdsError::InvalidData("zero width or height"));
        }
        let mip_map_count = mip_map_count_raw.max(1);
        let mut is_cube_map = caps2 & DDSCAPS2_CUBEMAP != 0;
        let is_volume = caps2 & DDSCAPS2_VOLUME != 0;
        let depth = if is_volume { depth.max(1) } else { 1 };

        // Resolve the format: DX10 header, FourCC, or bit masks.
        let mut array_size = 1;
        let mut dxgi_format = None;
        let mut srgb = false;
        let format = if pf_flags & DDPF_FOURCC != 0 && &fourcc == b"DX10" {
            if reader.remaining() < DDS_HEADER_DXT10_SIZE {
                return Err(DdsError::Truncated("DX10 header"));
            }
            let dxgi = reader.u32("DXGI format")?;
            let _resource_dimension = reader.u32("resource dimension")?;
            let misc_flag = reader.u32("misc flag")?;
            array_size = reader.u32("array size")?.max(1);
            let _misc_flags2 = reader.u32("misc flags 2")?;
            if misc_flag & DDS_RESOURCE_MISC_TEXTURECUBE != 0 {
                is_cube_map = true;
            }
            dxgi_format = Some(dxgi);
            let (format, is_srgb) = format_from_dxgi(dxgi)?;
            srgb = is_srgb;
            format
        } else if pf_flags & DDPF_FOURCC != 0 {
            format_from_fourcc(&fourcc)?
        } else {
            format_from_masks(pf_flags, rgb_bit_count, r_mask, g_mask, b_mask, a_mask)?
        };

        let image = DdsImage {
            width,
            height,
            depth,
            format,
            srgb,
            mip_map_count,
            array_size,
            is_cube_map,
            is_volume,
            dxgi_format,
            fourcc,
            data: reader.rest().to_vec(),
        };
        // The file must at least contain every mip level of the first
        // face/slice.
        let needed = image.mip_offset(mip_map_count);
        if image.data.len() < needed {
            return Err(DdsError::Truncated("surface data"));
        }
        Ok(image)
    }

    /// The raw surface data (all mip levels, faces, and slices),
    /// starting after the headers.
    pub fn surface_data(&self) -> &[u8] {
        &self.data
    }

    /// Dimensions of a mip level. Each level halves, minimum 1.
    pub fn mip_dimensions(&self, level: u32) -> (u32, u32) {
        let w = self.width.checked_shr(level).unwrap_or(0).max(1);
        let h = self.height.checked_shr(level).unwrap_or(0).max(1);
        (w, h)
    }

    /// Byte size of one mip level of one face/slice. Computed from
    /// the dimensions (per Microsoft's guidance), never taken from
    /// `dwPitchOrLinearSize`. Volume textures multiply by the level's
    /// depth (depth halves per level, minimum 1).
    pub fn mip_level_size(&self, level: u32) -> usize {
        let (w, h) = self.mip_dimensions(level);
        let d = self.depth.checked_shr(level).unwrap_or(0).max(1) as usize;
        let per_slice = match self.format {
            Format::Rgb(rgb) => {
                let pitch = (w as usize * rgb.bits_per_pixel as usize).div_ceil(8);
                pitch * h as usize
            }
            format => {
                let block_bytes = format.block_bytes().unwrap();
                let blocks_x = w.div_ceil(4).max(1) as usize;
                let blocks_y = h.div_ceil(4).max(1) as usize;
                blocks_x * blocks_y * block_bytes
            }
        };
        per_slice * d
    }

    /// Byte offset of a mip level within the surface data (first
    /// face/slice). Level `mip_map_count` gives the total size of one
    /// face/slice.
    pub fn mip_offset(&self, level: u32) -> usize {
        (0..level).map(|l| self.mip_level_size(l)).sum()
    }

    /// Decode one mip level of the first face/slice to RGBA8.
    /// Level 0 is the full-resolution image.
    pub fn decode(&self, level: u32) -> Result<RgbaImage> {
        if level >= self.mip_map_count {
            return Err(DdsError::InvalidData("mipmap level out of range"));
        }
        let (width, height) = self.mip_dimensions(level);
        let offset = self.mip_offset(level);
        let size = self.mip_level_size(level);
        let src = self
            .data
            .get(offset..offset + size)
            .ok_or(DdsError::Truncated("surface data"))?;
        let data = match self.format {
            Format::Rgb(rgb) => unpack_rgb(&rgb, src, width, height),
            format => {
                let bcn = format.bcn_format().unwrap();
                bcn_decode::decode_rgba8_to_vec(bcn, src, width as usize, height as usize)
                    .map_err(|_| DdsError::Decode("block data too small"))?
            }
        };
        Ok(RgbaImage {
            width,
            height,
            data,
        })
    }
}

/// Resolve a DXGI format value (from the DX10 header) to a
/// [`Format`] plus its sRGB flag. Typeless variants map to their
/// UNORM counterpart.
fn format_from_dxgi(dxgi: u32) -> Result<(Format, bool)> {
    let rgba8 = RgbFormat {
        bits_per_pixel: 32,
        r_mask: 0xff,
        g_mask: 0xff00,
        b_mask: 0xff0000,
        a_mask: 0xff000000,
        luminance: false,
        alpha_only: false,
    };
    let bgra8 = RgbFormat {
        bits_per_pixel: 32,
        r_mask: 0xff0000,
        g_mask: 0xff00,
        b_mask: 0xff,
        a_mask: 0xff000000,
        luminance: false,
        alpha_only: false,
    };
    let (format, srgb) = match dxgi {
        27 | 28 => (Format::Rgb(rgba8), false),
        29 => (Format::Rgb(rgba8), true),
        87 => (Format::Rgb(bgra8), false),
        70 | 71 => (Format::Bc1, false),
        72 => (Format::Bc1, true),
        73 | 74 => (Format::Bc2, false),
        75 => (Format::Bc2, true),
        76 | 77 => (Format::Bc3, false),
        78 => (Format::Bc3, true),
        79 | 80 => (Format::Bc4, false),
        81 => (Format::Bc4Snorm, false),
        82 | 83 => (Format::Bc5, false),
        84 => (Format::Bc5Snorm, false),
        95 => (Format::Bc6hUf16, false),
        96 => (Format::Bc6hSf16, false),
        97 | 98 => (Format::Bc7, false),
        99 => (Format::Bc7, true),
        _ => return Err(DdsError::UnsupportedDxgiFormat(dxgi)),
    };
    Ok((format, srgb))
}

/// Resolve a FourCC (from the classic header) to a [`Format`].
fn format_from_fourcc(fourcc: &[u8; 4]) -> Result<Format> {
    match fourcc {
        b"DXT1" => Ok(Format::Bc1),
        b"DXT2" | b"DXT3" => Ok(Format::Bc2),
        b"DXT4" | b"DXT5" => Ok(Format::Bc3),
        b"ATI1" | b"BC4U" => Ok(Format::Bc4),
        b"BC4S" => Ok(Format::Bc4Snorm),
        b"ATI2" | b"BC5U" => Ok(Format::Bc5),
        b"BC5S" => Ok(Format::Bc5Snorm),
        _ => Err(DdsError::UnsupportedFourcc(*fourcc)),
    }
}

/// Resolve an uncompressed pixel format from its flags and masks.
fn format_from_masks(
    pf_flags: u32,
    rgb_bit_count: u32,
    r_mask: u32,
    g_mask: u32,
    b_mask: u32,
    a_mask: u32,
) -> Result<Format> {
    let is_rgb = pf_flags & DDPF_RGB != 0;
    let is_luminance = pf_flags & DDPF_LUMINANCE != 0;
    let is_alpha = pf_flags & DDPF_ALPHA != 0;
    if !is_rgb && !is_luminance && !is_alpha {
        return Err(DdsError::InvalidData(
            "unsupported pixel format flags (not RGB, luminance, or alpha)",
        ));
    }
    if !matches!(rgb_bit_count, 8 | 16 | 24 | 32) {
        return Err(DdsError::InvalidData("unsupported RGB bit count"));
    }
    let alpha_only = is_alpha && !is_rgb && !is_luminance;
    Ok(Format::Rgb(RgbFormat {
        bits_per_pixel: rgb_bit_count,
        r_mask,
        g_mask,
        b_mask,
        a_mask: if pf_flags & DDPF_ALPHAPIXELS != 0 || is_alpha {
            a_mask
        } else {
            0
        },
        luminance: is_luminance,
        alpha_only,
    }))
}

/// Unpack uncompressed pixels to RGBA8, using the bit masks. Each
/// channel is shifted out of its mask and scaled to 8 bits.
fn unpack_rgb(format: &RgbFormat, src: &[u8], width: u32, height: u32) -> Vec<u8> {
    let bytes_per_pixel = (format.bits_per_pixel / 8) as usize;
    let pitch = (width as usize * format.bits_per_pixel as usize).div_ceil(8);
    let mut out = vec![0u8; width as usize * height as usize * 4];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let offset = y * pitch + x * bytes_per_pixel;
            let raw = match bytes_per_pixel {
                1 => u32::from(src[offset]),
                2 => u16::from_le_bytes(src[offset..offset + 2].try_into().unwrap()) as u32,
                3 => u32::from_le_bytes([src[offset], src[offset + 1], src[offset + 2], 0]),
                _ => u32::from_le_bytes(src[offset..offset + 4].try_into().unwrap()),
            };
            let (r, g, b, a) = if format.alpha_only {
                (255, 255, 255, scale_channel(raw, format.a_mask))
            } else if format.luminance {
                let l = scale_channel(raw, format.r_mask);
                let a = if format.a_mask != 0 {
                    scale_channel(raw, format.a_mask)
                } else {
                    255
                };
                (l, l, l, a)
            } else {
                (
                    scale_channel(raw, format.r_mask),
                    scale_channel(raw, format.g_mask),
                    scale_channel(raw, format.b_mask),
                    if format.a_mask != 0 {
                        scale_channel(raw, format.a_mask)
                    } else {
                        255
                    },
                )
            };
            let o = (y * width as usize + x) * 4;
            out[o] = r;
            out[o + 1] = g;
            out[o + 2] = b;
            out[o + 3] = a;
        }
    }
    out
}

/// Extract one channel from a packed pixel value and scale it to 8
/// bits. A zero mask means the channel is absent (0).
fn scale_channel(raw: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = mask.count_ones();
    let value = (raw & mask) >> shift;
    if bits >= 8 {
        (value >> (bits - 8)) as u8
    } else {
        ((value * 255) / ((1u32 << bits) - 1)) as u8
    }
}

/// A bounds-checked cursor over the DDS bytes.
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

    fn rest(&self) -> &'a [u8] {
        &self.bytes[self.pos..]
    }

    fn take(&mut self, len: usize, what: &'static str) -> Result<&'a [u8]> {
        if self.remaining() < len {
            return Err(DdsError::Truncated(what));
        }
        let slice = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn u32(&mut self, what: &'static str) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_scaling() {
        // 5-bit max -> 255, 6-bit max -> 255, 1-bit -> 255.
        assert_eq!(scale_channel(0xF800, 0xF800), 255);
        assert_eq!(scale_channel(0x07E0, 0x07E0), 255);
        assert_eq!(scale_channel(0x8000, 0x8000), 255);
        assert_eq!(scale_channel(0x0000, 0xF800), 0);
        assert_eq!(scale_channel(0x1234, 0), 0);
        // 8-bit channel passes through.
        assert_eq!(scale_channel(0xAB, 0xFF), 0xAB);
        // 16-bit channel takes the top 8 bits.
        assert_eq!(scale_channel(0xABCD, 0xFFFF), 0xAB);
    }
}
