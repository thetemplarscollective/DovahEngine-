# DDS — DirectDraw Surface

Source: Microsoft Learn, "Programming Guide for DDS"
(`https://learn.microsoft.com/en-us/windows/win32/direct3ddds/dx-graphics-dds-pguide`),
the `DDS_HEADER`, `DDS_PIXELFORMAT`, and `DDS_HEADER_DXT10` reference
pages, "Block Compression (Direct3D 10)", and the `DXGI_FORMAT` enum
reference. Notes below are our own summary of those public docs.

## What it is

Microsoft's texture container. Skyrim's textures are mostly
BC-compressed (BC1/BC3/BC7) with mipmaps; some are uncompressed. A
DDS file is a magic number, a header (plus an optional DX10 extension
header), then the surface data. All numbers are little-endian.

## Layout

```
DWORD            dwMagic;    // "DDS " (0x20534444)
DDS_HEADER       header;     // 124 bytes
DDS_HEADER_DXT10 header10;   // 20 bytes, only when FourCC == "DX10"
BYTE             bdata[];    // surface data: mip levels, then cube faces / array slices
```

Surface data starts at offset 128 (classic) or 148 (DX10 header).
A reader validates: file at least 128 bytes, magic `0x20534444`,
`dwSize` 124, pixel-format `dwSize` 32.

## DDS_HEADER (124 bytes)

| Offset | Field | Notes |
| --- | --- | --- |
| 0 | dwSize | Must be 124 |
| 4 | dwFlags | Which members hold valid data (below) |
| 8 | dwHeight | Surface height in pixels |
| 12 | dwWidth | Surface width in pixels |
| 16 | dwPitchOrLinearSize | Pitch (uncompressed) or top-level size (compressed). Unreliable across writers — compute it instead (see "Mipmaps and layout") |
| 20 | dwDepth | Depth of a volume texture, otherwise unused |
| 24 | dwMipMapCount | Number of mipmap levels (0 means 1) |
| 28 | dwReserved1[11] | Unused |
| 72 | ddspf | `DDS_PIXELFORMAT`, 32 bytes (below) |
| 104 | dwCaps | Surface complexity (below) |
| 108 | dwCaps2 | Cube map / volume flags (below) |
| 112 | dwCaps3 | Unused |
| 116 | dwCaps4 | Unused |
| 120 | dwReserved2 | Unused |

`dwFlags` (`DDSD_*`): CAPS `0x1`, HEIGHT `0x2`, WIDTH `0x4`, PITCH
`0x8`, PIXELFORMAT `0x1000`, MIPMAPCOUNT `0x20000`, LINEARSIZE
`0x80000`, DEPTH `0x800000`. Microsoft warns not to rely on the
CAPS, PIXELFORMAT, and MIPMAPCOUNT flags being set — some writers
omit them.

`dwCaps`: COMPLEX `0x8` (more than one surface), MIPMAP `0x400000`,
TEXTURE `0x1000` (required, but do not rely on it).

`dwCaps2`: CUBEMAP `0x200`, CUBEMAP_POSITIVEX `0x400`,
CUBEMAP_NEGATIVEX `0x800`, CUBEMAP_POSITIVEY `0x1000`,
CUBEMAP_NEGATIVEY `0x2000`, CUBEMAP_POSITIVEZ `0x4000`,
CUBEMAP_NEGATIVEZ `0x8000`, VOLUME `0x200000`.

## DDS_PIXELFORMAT (32 bytes)

| Offset | Field | Notes |
| --- | --- | --- |
| 0 | dwSize | Must be 32 |
| 4 | dwFlags | What the pixel data is (below) |
| 8 | dwFourCC | Four-character code for compressed/special formats |
| 12 | dwRGBBitCount | Bits per pixel (uncompressed formats) |
| 16 | dwRBitMask | Red (or luminance, or Y) mask |
| 20 | dwGBitMask | Green (or U) mask |
| 24 | dwBBitMask | Blue (or V) mask |
| 28 | dwABitMask | Alpha mask |

`dwFlags` (`DDPF_*`): ALPHAPIXELS `0x1` (texture has alpha data),
ALPHA `0x2` (alpha-only legacy), FOURCC `0x4` (dwFourCC is valid),
RGB `0x40` (uncompressed RGB), YUV `0x200`, LUMINANCE `0x20000`
(single-channel, dwRBitMask is the channel; combinable with
ALPHAPIXELS for a two-channel file).

## DDS_HEADER_DXT10 (20 bytes)

Present only when `dwFourCC` is `"DX10"`. Carries formats the classic
header cannot express (BC6H/BC7, sRGB, float, texture arrays):

| Field | Notes |
| --- | --- |
| dxgiFormat | A `DXGI_FORMAT` value (below) |
| resourceDimension | `D3D10_RESOURCE_DIMENSION`: 1 buffer, 2 1D, 3 2D, 4 3D |
| miscFlag | `DDS_RESOURCE_MISC_TEXTURECUBE` = `0x4` for cube maps |
| arraySize | Number of array elements (1 for a plain texture) |
| miscFlags2 | Alpha mode (straight / premultiplied / opaque / custom) |

## Formats

### FourCC (classic header, `DDPF_FOURCC`)

| FourCC | Format | Block size |
| --- | --- | --- |
| `DXT1` | BC1 | 8 bytes |
| `DXT2`, `DXT3` | BC2 | 16 bytes |
| `DXT4`, `DXT5` | BC3 | 16 bytes |
| `ATI1`, `BC4U` | BC4 (UNORM) | 8 bytes |
| `BC4S` | BC4 (SNORM) | 8 bytes |
| `ATI2`, `BC5U` | BC5 (UNORM) | 16 bytes |
| `BC5S` | BC5 (SNORM) | 16 bytes |
| `DX10` | See the DX10 header | — |

### DXGI_FORMAT (DX10 header)

| Value | Format |
| --- | --- |
| 28 / 29 | R8G8B8A8_UNORM / _UNORM_SRGB |
| 87 | B8G8R8A8_UNORM |
| 71 / 72 | BC1_UNORM / _UNORM_SRGB |
| 74 / 75 | BC2_UNORM / _UNORM_SRGB |
| 77 / 78 | BC3_UNORM / _UNORM_SRGB |
| 80 / 81 | BC4_UNORM / _SNORM |
| 83 / 84 | BC5_UNORM / _SNORM |
| 95 / 96 | BC6H_UF16 / _SF16 |
| 98 / 99 | BC7_UNORM / _UNORM_SRGB |

The `_SRGB` variants decode to the same bytes as the `_UNORM`
variants — sRGB is an interpretation, not a different encoding.
The reader records `srgb` on the image and leaves color-space
handling to the renderer.

### Uncompressed (no FourCC)

RGB data is described by bit masks. Common layouts:

| Format | dwRGBBitCount | R mask | G mask | B mask | A mask |
| --- | --- | --- | --- | --- | --- |
| A8B8G8R8 | 32 | `0xff` | `0xff00` | `0xff0000` | `0xff000000` |
| A8R8G8B8 | 32 | `0xff0000` | `0xff00` | `0xff` | `0xff000000` |
| R8G8B8 | 24 | `0xff0000` | `0xff00` | `0xff` | — |
| R5G6B5 | 16 | `0xf800` | `0x7e0` | `0x1f` | — |
| A1R5G5B5 | 16 | `0x7c00` | `0x3e0` | `0x1f` | `0x8000` |
| A8 | 8 | — | — | — | `0xff` |
| L8 | 8 | `0xff` | — | — | — |

The reader unpacks any contiguous-mask layout: each channel is
shifted out of its mask and scaled to 8 bits. Luminance files
(`DDPF_LUMINANCE`) copy the single channel to R, G, and B.
Alpha-only files decode to white RGB with the stored alpha.

## Block compression

All BC formats work on 4×4 texel blocks. Textures are stored as
blocks in row-major order; a mip level smaller than 4×4 still stores
whole blocks (see "Mipmaps and layout").

- **BC1** (8 bytes/block): two RGB565 endpoints + sixteen 2-bit
  indices. If color0 > color1 the four palette colors are the two
  endpoints plus 2/3–1/3 and 1/3–2/3 blends (all opaque). Otherwise
  the palette is the two endpoints, their 50/50 blend, and
  transparent black — 1-bit alpha.
- **BC2** (16 bytes): 64 bits of explicit 4-bit alpha (one nibble per
  texel), then a BC1 color block.
- **BC3** (16 bytes): an alpha block (two 8-bit endpoints + sixteen
  3-bit indices; six interpolated values when alpha0 > alpha1,
  otherwise four interpolated values plus 0 and 255), then a BC1
  color block.
- **BC4** (8 bytes): one channel, using the BC3 alpha-block scheme.
- **BC5** (16 bytes): two channels, two BC4 blocks back to back
  (used for normal maps: red = X, green = Y).
- **BC6H** (16 bytes): HDR color in half floats, 14 modes; unsigned
  (UF16) and signed (SF16) variants.
- **BC7** (16 bytes): the highest-quality BC format, 8 modes, RGB
  and RGBA.

BC1–BC5 have no bit-exact 8-bit definition in the specs (they give
real values); BC6H and BC7 are bit-exact.

## Mipmaps and layout

- Mip level dimensions halve each level, minimum 1. A 60×40 texture
  has mips 60×40, 30×20, 15×10, 7×5, 3×2, 1×1.
- Block-compressed pitch (bytes per row of blocks):
  `max(1, (width + 3) / 4) × block-size`, where block-size is 8 for
  BC1/BC4 and 16 for the other BC formats.
- Uncompressed pitch: `(width × bits-per-pixel + 7) / 8`.
- Mip level size (BC) = blocks-wide × blocks-high × block-size;
  a level smaller than 4×4 still occupies one full block (physical
  size is rounded up to a multiple of 4; sampling uses the virtual
  size).
- Data order: all mip levels of face/slice 0, then face/slice 1, and
  so on. Cube-map faces run +X, −X, +Y, −Y, +Z, −Z.
- Microsoft's guidance, followed here: compute pitch and sizes from
  the dimensions; do not trust `dwPitchOrLinearSize`.

## Cube maps, volumes, arrays

Cube maps carry six faces (flagged in `dwCaps2` or via the DX10
`miscFlag`); volume textures carry depth slices (`dwDepth`,
`DDSCAPS2_VOLUME`); DX10 textures can be arrays (`arraySize`). The
M4 reader exposes these header facts and decodes the first
face/slice; full cube/volume/array decode is M6 work, when the
renderer needs environment maps.

## Reader notes (dovah-dds)

- The header and the DX10 extension header are parsed; the format is
  resolved from the FourCC or the DXGI format value.
- Uncompressed RGB(A), luminance, and alpha-only data are unpacked
  by hand from the bit masks.
- **BC decoding uses the `bcn_decode` crate (v0.1.0).** It is pure
  Rust, has zero dependencies and no `unsafe`, is dual-licensed MIT
  OR Apache-2.0, and was implemented from Microsoft's D3D10/11
  block-compression documentation and the Khronos Data Format
  Specification 1.3.1. Rationale: BC7's eight modes and partition
  tables are too error-prone to hand-roll reliably, and hand-rolling
  BC1/BC3 would duplicate a spec-derived crate. Only the DDS
  container (headers, mipmap layout) and the uncompressed unpacking
  are hand-written here.
- BC1 decodes with 1-bit alpha semantics (`Bc1Alpha`): a block in
  three-color mode yields transparent black for index 3, matching
  Direct3D.
- BC6H decodes to RGBA8 clamped to [0, 1] — HDR above 1.0 is lost
  in the 8-bit path. The crate also offers an f32 API that keeps the
  full half-float range; the renderer can switch to it later.
- `decode(level)` returns RGBA8 bytes for one mip level of the first
  face/slice. Mip level 0 is the full-resolution image.
- Android ETC2/ASTC transcoding is a Phase 4 concern, not a Phase 1
  one: the reader decodes to RGBA8, and the phone build will decide
  how to upload or transcode that.
- Real-data check against Dylan's textures still pending.
