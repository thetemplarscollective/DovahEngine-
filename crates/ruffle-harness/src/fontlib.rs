//! Runtime `gfxfontlib.swf` synthesis (M4).
//!
//! Skyrim's menus import their fonts from `gfxfontlib.swf`, a file
//! vanilla Skyrim does not ship — the game synthesizes it at load
//! time from `interface/fonts_en.swf` (see the feasibility study
//! §2.1). The study built one by hand for the harness
//! (`interface-extract/tools/ruffle-test/gfxfontlib.swf`); the
//! engine cannot use a hardcoded dev path, so this module rebuilds
//! the same file from the game's own `fonts_en.swf` bytes at runtime.
//!
//! Structure (verified against the study's reference file, which is
//! FWS, version 10, 487,761 bytes):
//!
//! * SWF header: `FWS` + version (copied from the input) + total
//!   length, then the input's RECT, frame rate, and frame count,
//!   copied verbatim.
//! * Every `DefineFont3` tag (tag 75) from the input, copied
//!   VERBATIM (full tag bytes, including the long-form header), in
//!   file order.
//! * One `ExportAssets` tag (tag 56) exporting the 15 `$` names the
//!   menus import, mapped from (font face, bold flag) — see
//!   [`FONTLIB_EXPORTS`].
//! * `ShowFrame` + `End`.
//!
//! Which `fonts_en.swf` the engine feeds in matters: the game's
//! Patch.bsa carries a NEWER copy (16 fonts, IDs 1–27, including
//! "Controller Buttons" and Cyrillic faces) than Interface.bsa's
//! (12 fonts, IDs 1,3,…,23). The engine loads the Patch copy via
//! `AssetProvider::read_file_latest`. The synthesis maps by NAME,
//! not ID, so it is correct for either file — the reference
//! byte-identical test pins the Interface input, and the Patch
//! structural test pins the shifted IDs.

use std::fmt;
use std::io::Read;

/// (export name, font face, bold flag), in the reference file's
/// export order. The face names come from
/// `interface-extract/interface/fontconfig.txt`; the bold flag is
/// bit 0x01 of the DefineFont3 flags byte (offset 2 in the tag body).
/// Two faces export under two names each (Futura CondensedLight →
/// `$EverywhereFont` + `$DialogueFont`; Futura Condensed (regular) →
/// `$EverywhereMediumFont` + `$StartMenuFont`; Handwritten_Bold →
/// `$HandwrittenFont` + `$HandwrittenBold`). `$ConsoleFont` maps to
/// Arial, a device font — it is not in the SWF and is skipped.
pub const FONTLIB_EXPORTS: [(&str, &str, bool); 15] = [
    ("$EverywhereFont", "Futura CondensedLight", false),
    ("$EverywhereMediumFont", "Futura Condensed", false),
    ("$EverywhereBoldFont", "Futura Condensed", true),
    ("$StartMenuFont", "Futura Condensed", false),
    ("$DialogueFont", "Futura CondensedLight", false),
    ("$DragonFont", "Dragon_script", false),
    ("$SkyrimBooks", "SkyrimBooks_Gaelic", false),
    ("$HandwrittenFont", "SkyrimBooks_Handwritten_Bold", false),
    ("$HandwrittenBold", "SkyrimBooks_Handwritten_Bold", false),
    ("$FalmerFont", "Falmer", false),
    ("$DwemerFont", "Dwemer", false),
    ("$DaedricFont", "Daedric", false),
    ("$MageScriptFont", "Mage Script", false),
    ("$SkyrimSymbolsFont", "SkyrimSymbols", false),
    (
        "$SkyrimBooks_UnreadableFont",
        "SkyrimBooks_Unreadable",
        false,
    ),
];

/// Errors from [`synthesize_fontlib`]. All are loud failures: a
/// fontlib with a missing or misidentified font would render menus
/// with blank text, so the engine treats any error as "no Ruffle
/// HUD" and falls back to the Bevy HUD.
#[derive(Debug)]
pub struct FontlibError(pub String);

impl fmt::Display for FontlibError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fontlib synthesis failed: {}", self.0)
    }
}

impl std::error::Error for FontlibError {}

/// One `DefineFont3` tag found in the input: its verbatim bytes
/// (full tag, header included) plus the parsed identity used for
/// the export mapping.
struct FoundFont {
    /// Full tag bytes, header included — copied verbatim.
    tag_bytes: Vec<u8>,
    /// The FontID from the tag body.
    id: u16,
    /// Face name, trailing NUL stripped.
    name: String,
    /// Bold flag (bit 0x01 of the flags byte).
    bold: bool,
}

/// Build `gfxfontlib.swf` from the game's `fonts_en.swf` bytes.
///
/// Accepts FWS (uncompressed) and CWS (zlib-compressed) inputs.
/// ZWS (LZMA) is rejected — the game does not ship it and adding an
/// LZMA dependency for a file that never occurs is not worth it.
pub fn synthesize_fontlib(fonts_en_swf: &[u8]) -> Result<Vec<u8>, FontlibError> {
    if fonts_en_swf.len() < 8 {
        return Err(FontlibError(
            "input shorter than the 8-byte SWF header".into(),
        ));
    }
    let sig = &fonts_en_swf[0..3];
    let version = fonts_en_swf[3];
    let body: Vec<u8> = match sig {
        b"FWS" => fonts_en_swf[8..].to_vec(),
        b"CWS" => {
            let mut decoder = flate2::read::ZlibDecoder::new(&fonts_en_swf[8..]);
            let mut out = Vec::with_capacity(fonts_en_swf.len() * 2);
            decoder
                .read_to_end(&mut out)
                .map_err(|e| FontlibError(format!("zlib decompress failed: {e}")))?;
            out
        }
        b"ZWS" => {
            return Err(FontlibError("ZWS (LZMA) input is not supported".into()));
        }
        _ => {
            return Err(FontlibError(format!(
                "bad SWF signature: {:?}",
                String::from_utf8_lossy(sig)
            )));
        }
    };

    // RECT: first byte's top 5 bits are nbits; the rect occupies
    // (5 + 4*nbits) bits, rounded up to whole bytes.
    if body.is_empty() {
        return Err(FontlibError("empty SWF body".into()));
    }
    let nbits = (body[0] >> 3) as usize;
    let rect_len = (5 + 4 * nbits).div_ceil(8);
    if body.len() < rect_len + 4 {
        return Err(FontlibError(
            "SWF body too short for RECT + frame header".into(),
        ));
    }
    let rect = &body[..rect_len];
    // Frame rate (u16 LE, 8.8 fixed) + frame count (u16 LE), copied.
    let frame_header = &body[rect_len..rect_len + 4];

    // Walk the tags, collecting DefineFont3 (tag 75) verbatim.
    let mut fonts: Vec<FoundFont> = Vec::new();
    let mut off = rect_len + 4;
    while off < body.len() {
        if off + 2 > body.len() {
            return Err(FontlibError("truncated tag header".into()));
        }
        let header = u16::from_le_bytes([body[off], body[off + 1]]);
        let code = header >> 6;
        let mut len = (header & 0x3F) as usize;
        let mut hdr_len = 2usize;
        if len == 0x3F {
            if off + 6 > body.len() {
                return Err(FontlibError("truncated long-form tag header".into()));
            }
            len = u32::from_le_bytes([body[off + 2], body[off + 3], body[off + 4], body[off + 5]])
                as usize;
            hdr_len = 6;
        }
        if off + hdr_len + len > body.len() {
            return Err(FontlibError(format!(
                "truncated tag body (code {code}, len {len})"
            )));
        }
        if code == 75 {
            let tag_start = off;
            let tag_end = off + hdr_len + len;
            let tag_body = &body[off + hdr_len..tag_end];
            fonts.push(parse_font_tag(&body[tag_start..tag_end], tag_body)?);
        }
        off += hdr_len + len;
        if code == 0 {
            break;
        }
    }
    if fonts.is_empty() {
        return Err(FontlibError("no DefineFont3 tags found".into()));
    }

    // Build the ExportAssets body from the (name, bold) map.
    let mut export_body: Vec<u8> = Vec::new();
    export_body.extend_from_slice(&(FONTLIB_EXPORTS.len() as u16).to_le_bytes());
    for (export_name, face, bold) in FONTLIB_EXPORTS {
        let font = fonts
            .iter()
            .find(|f| f.name == face && f.bold == bold)
            .ok_or_else(|| {
                FontlibError(format!(
                    "font {face:?} (bold={bold}) not found in fonts_en.swf"
                ))
            })?;
        export_body.extend_from_slice(&font.id.to_le_bytes());
        export_body.extend_from_slice(export_name.as_bytes());
        export_body.push(0);
    }

    // Assemble: FWS header + RECT + frame header + font tags +
    // ExportAssets + ShowFrame + End. The total length is computed
    // from the parts.
    let export_tag_len = if export_body.len() < 63 {
        2 + export_body.len()
    } else {
        6 + export_body.len()
    };
    let fonts_len: usize = fonts.iter().map(|f| f.tag_bytes.len()).sum();
    let total = 8 + rect_len + 4 + fonts_len + export_tag_len + 2 + 2;

    let mut out: Vec<u8> = Vec::with_capacity(total);
    out.extend_from_slice(b"FWS");
    out.push(version);
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(rect);
    out.extend_from_slice(frame_header);
    for font in &fonts {
        out.extend_from_slice(&font.tag_bytes);
    }
    // ExportAssets tag (code 56).
    if export_body.len() < 63 {
        let header = (56u16 << 6) | (export_body.len() as u16);
        out.extend_from_slice(&header.to_le_bytes());
    } else {
        let header = (56u16 << 6) | 0x3F;
        out.extend_from_slice(&header.to_le_bytes());
        out.extend_from_slice(&(export_body.len() as u32).to_le_bytes());
    }
    out.extend_from_slice(&export_body);
    // ShowFrame (tag 1, no body).
    out.extend_from_slice(&(1u16 << 6).to_le_bytes());
    // End (tag 0, no body).
    out.extend_from_slice(&0u16.to_le_bytes());
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

/// Parse one DefineFont3 tag's identity from its body.
///
/// Body layout: FontID u16 @0, flags u8 @2 (bit 0x01 bold, 0x02
/// italic), language u8 @3, name length u8 @4, name bytes @5 (the
/// length INCLUDES a trailing NUL, which is stripped for matching).
fn parse_font_tag(tag_bytes: &[u8], tag_body: &[u8]) -> Result<FoundFont, FontlibError> {
    if tag_body.len() < 5 {
        return Err(FontlibError("DefineFont3 tag too short".into()));
    }
    let id = u16::from_le_bytes([tag_body[0], tag_body[1]]);
    let flags = tag_body[2];
    let name_len = tag_body[4] as usize;
    if tag_body.len() < 5 + name_len {
        return Err(FontlibError(format!(
            "DefineFont3 tag (id {id}) truncates its name"
        )));
    }
    let mut name = String::from_utf8_lossy(&tag_body[5..5 + name_len]).into_owned();
    // The name carries a trailing NUL, included in the length.
    if name.ends_with('\0') {
        name.pop();
    }
    Ok(FoundFont {
        tag_bytes: tag_bytes.to_vec(),
        id,
        name,
        bold: flags & 0x01 != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal synthetic `fonts_en.swf` in memory: FWS,
    /// version 10, a 1×1 RECT, 24 fps, 1 frame, and the two Futura
    /// Condensed faces the export map needs for its first three
    /// entries — plus every other mapped face, so the synthesis
    /// succeeds. Tag bodies are minimal (header fields + 4 dummy
    /// bytes); the synthesis copies them verbatim, so the glyph data
    /// content is irrelevant here.
    fn synthetic_fonts_en() -> Vec<u8> {
        let faces: [(&str, bool); 12] = [
            ("Futura CondensedLight", false),
            ("Futura Condensed", false),
            ("Futura Condensed", true),
            ("Dragon_script", false),
            ("SkyrimBooks_Gaelic", false),
            ("SkyrimBooks_Handwritten_Bold", false),
            ("Falmer", false),
            ("Dwemer", false),
            ("Daedric", false),
            ("Mage Script", false),
            ("SkyrimSymbols", false),
            ("SkyrimBooks_Unreadable", false),
        ];
        let mut body: Vec<u8> = Vec::new();
        // RECT: nbits 0, all zeroes (1 byte: the 5-bit nbits field
        // alone, 5 bits rounded up to one byte).
        body.push(0x00);
        // fps 24.0 (8.8 fixed: 24<<8), 1 frame.
        body.extend_from_slice(&(24u16 << 8).to_le_bytes());
        body.extend_from_slice(&1u16.to_le_bytes());
        for (i, (face, bold)) in faces.iter().enumerate() {
            let id = (i as u16) + 1;
            let mut tag_body: Vec<u8> = Vec::new();
            tag_body.extend_from_slice(&id.to_le_bytes());
            tag_body.push(if *bold { 0x01 } else { 0x00 });
            tag_body.push(0); // language
            let name_bytes = format!("{face}\0");
            tag_body.push(name_bytes.len() as u8);
            tag_body.extend_from_slice(name_bytes.as_bytes());
            tag_body.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]); // dummy glyph data
            let header = (75u16 << 6) | (tag_body.len() as u16);
            body.extend_from_slice(&header.to_le_bytes());
            body.extend_from_slice(&tag_body);
        }
        body.extend_from_slice(&((1u16 << 6) as u16).to_le_bytes()); // ShowFrame
        body.extend_from_slice(&0u16.to_le_bytes()); // End
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"FWS");
        out.push(10);
        out.extend_from_slice(&((8 + body.len()) as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// Read the ExportAssets exports from a synthesized fontlib:
    /// returns (export name, font ID) pairs in file order.
    fn read_exports(fontlib: &[u8]) -> Vec<(String, u16)> {
        // Skip the 8-byte header, the RECT, and the frame header.
        let nbits = (fontlib[8] >> 3) as usize;
        let rect_len = (5 + 4 * nbits + 7) / 8;
        let mut off = 8 + rect_len + 4;
        while off < fontlib.len() {
            let header = u16::from_le_bytes([fontlib[off], fontlib[off + 1]]);
            let code = header >> 6;
            let mut len = (header & 0x3F) as usize;
            let mut hdr_len = 2usize;
            if len == 0x3F {
                len = u32::from_le_bytes([
                    fontlib[off + 2],
                    fontlib[off + 3],
                    fontlib[off + 4],
                    fontlib[off + 5],
                ]) as usize;
                hdr_len = 6;
            }
            if code == 56 {
                let body = &fontlib[off + hdr_len..off + hdr_len + len];
                let count = u16::from_le_bytes([body[0], body[1]]) as usize;
                let mut exports = Vec::with_capacity(count);
                let mut p = 2usize;
                for _ in 0..count {
                    let id = u16::from_le_bytes([body[p], body[p + 1]]);
                    p += 2;
                    let end = body[p..].iter().position(|b| *b == 0).unwrap() + p;
                    let name = String::from_utf8_lossy(&body[p..end]).into_owned();
                    p = end + 1;
                    exports.push((name, id));
                }
                return exports;
            }
            off += hdr_len + len;
            if code == 0 {
                break;
            }
        }
        panic!("no ExportAssets tag found");
    }

    #[test]
    fn synthetic_input_maps_by_name_and_bold() {
        let input = synthetic_fonts_en();
        let out = synthesize_fontlib(&input).expect("synthesis");
        // FWS, version copied, length field == file length.
        assert_eq!(&out[0..3], b"FWS");
        assert_eq!(out[3], 10);
        assert_eq!(
            u32::from_le_bytes([out[4], out[5], out[6], out[7]]) as usize,
            out.len()
        );
        let exports = read_exports(&out);
        assert_eq!(exports.len(), 15);
        // The synthetic IDs are 1..=12 in face order: Futura
        // CondensedLight = 1, Futura Condensed regular = 2, bold = 3.
        assert_eq!(exports[0], ("$EverywhereFont".to_string(), 1));
        assert_eq!(exports[1], ("$EverywhereMediumFont".to_string(), 2));
        assert_eq!(exports[2], ("$EverywhereBoldFont".to_string(), 3));
        assert_eq!(exports[3], ("$StartMenuFont".to_string(), 2));
        assert_eq!(exports[4], ("$DialogueFont".to_string(), 1));
        // The dummy glyph bytes survive verbatim.
        assert!(out.windows(4).any(|w| w == [0xAA, 0xBB, 0xCC, 0xDD]));
    }

    #[test]
    fn missing_font_is_a_loud_error() {
        // Feed an input with only one font tag: the other 14
        // exports cannot resolve.
        let mut body: Vec<u8> = Vec::new();
        body.push(0x00);
        body.extend_from_slice(&(24u16 << 8).to_le_bytes());
        body.extend_from_slice(&1u16.to_le_bytes());
        let mut tag_body: Vec<u8> = Vec::new();
        tag_body.extend_from_slice(&1u16.to_le_bytes());
        tag_body.push(0);
        tag_body.push(0);
        tag_body.push(b"Futura CondensedLight\0".len() as u8);
        tag_body.extend_from_slice(b"Futura CondensedLight\0");
        let header = (75u16 << 6) | (tag_body.len() as u16);
        body.extend_from_slice(&header.to_le_bytes());
        body.extend_from_slice(&tag_body);
        body.extend_from_slice(&((1u16 << 6) as u16).to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        let mut single: Vec<u8> = Vec::new();
        single.extend_from_slice(b"FWS");
        single.push(10);
        single.extend_from_slice(&((8 + body.len()) as u32).to_le_bytes());
        single.extend_from_slice(&body);
        let err = synthesize_fontlib(&single).expect_err("one font cannot satisfy 15 exports");
        assert!(
            err.0.contains("Futura Condensed"),
            "error names the missing face: {err}"
        );
    }

    #[test]
    fn zws_and_garbage_are_rejected() {
        assert!(synthesize_fontlib(b"ZWS\x0a\x08\x00\x00\x00").is_err());
        assert!(synthesize_fontlib(b"not a swf at all").is_err());
        assert!(synthesize_fontlib(b"FWS").is_err());
    }

    /// The study's reference file, rebuilt from the Interface
    /// `fonts_en.swf`, must be BYTE-IDENTICAL — this pins the
    /// assembly (header, RECT, tag order, export order) against the
    /// file M1–M3 proved. Skipped (not failed) when the study files
    /// are absent, e.g. a bare checkout without `interface-extract/`.
    #[test]
    fn reference_fontlib_is_byte_identical() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let input_path = manifest.join("../../interface-extract/interface/fonts_en.swf");
        let ref_path = manifest.join("../../interface-extract/tools/ruffle-test/gfxfontlib.swf");
        if !input_path.exists() || !ref_path.exists() {
            return;
        }
        let input = std::fs::read(&input_path).expect("read fonts_en.swf");
        let reference = std::fs::read(&ref_path).expect("read reference gfxfontlib.swf");
        let out = synthesize_fontlib(&input).expect("synthesis");
        assert_eq!(out.len(), reference.len(), "length differs");
        assert_eq!(
            out, reference,
            "synthesized fontlib differs from the reference"
        );
    }

    /// The Patch `fonts_en.swf` (the one the game loads) has 16
    /// fonts at shifted IDs — the map must resolve by name, so
    /// `$EverywhereFont` lands on Patch's Futura CondensedLight
    /// (ID 5), not Interface's (ID 1). Skipped when the Patch
    /// extract is absent.
    #[test]
    fn patch_fonts_resolve_by_name() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let input_path = manifest.join("../../interface-extract/interface/fonts_en-patch.swf");
        if !input_path.exists() {
            return;
        }
        let input = std::fs::read(&input_path).expect("read Patch fonts_en.swf");
        let out = synthesize_fontlib(&input).expect("synthesis");
        let exports = read_exports(&out);
        assert_eq!(exports.len(), 15);
        assert_eq!(exports[0], ("$EverywhereFont".to_string(), 5));
        assert_eq!(exports[1], ("$EverywhereMediumFont".to_string(), 7));
        assert_eq!(exports[2], ("$EverywhereBoldFont".to_string(), 9));
        assert_eq!(exports[5], ("$DragonFont".to_string(), 11));
    }
}
