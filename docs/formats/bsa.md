# BSA — Bethesda Softworks Archive

Source: UESP wiki, "Skyrim Mod:Archive File Format"
(`https://en.uesp.net/wiki/Skyrim_Mod:Archive_File_Format`).
Notes below are our own summary of that public spec.

## Versions

- **v104 (0x68)** — Skyrim Legendary Edition, Fallout 3, Fallout: New Vegas.
  Compressed files use **zlib**.
- **v105 (0x69)** — Skyrim Special Edition. Compressed files use **LZ4
  frame format** (magic `04 22 4D 18`; the uncompressed size is stored
  separately). NOT raw LZ4 blocks — block decoding was tried first and
  failed on every file of the real Textures0.bsa (2026-11-13).
- v103 is Oblivion; not a target.

## Layout (all little-endian unless the Xbox flag is set)

```
Header (36 bytes)
Folder records (folderCount entries)
File-record blocks (one per folder: folder name + its file records)
File-name block (all file names, NUL-terminated, in file-record order)
File data blobs
```

## Header (36 bytes)

| Field | Size | Notes |
| --- | --- | --- |
| Magic | 4 | `"BSA\0"` |
| Version | u32 | 104 or 105 |
| Folder-record offset | u32 | Always 36 |
| Archive flags | u32 | See below |
| Folder count | u32 | |
| File count | u32 | |
| Total folder-name length | u32 | Includes NULs, excludes length bytes |
| Total file-name length | u32 | Includes NULs |
| File flags | u16 | Content hints (meshes, textures, ...) |
| Padding | u16 | |

Archive flags: `0x1` directory names present, `0x2` file names present,
`0x4` compressed by default, `0x8` retain directory names, `0x10` retain
file names, `0x20` retain file-name offsets, `0x40` Xbox 360 archive
(big-endian — we refuse these), `0x80` retain strings during startup,
`0x100` embed file names in data blocks, `0x200` XMem codec (Xbox only).
Bits 0 and 1 are set in all official PC archives.

## Folder record

- v104: 16 bytes — name hash (u64), file count (u32), offset (u32).
- v105: 24 bytes — name hash (u64), file count (u32), padding (u32),
  offset (u32), padding (u32). The extra padding is the reason for the
  v104/v105 split: Bethesda serializes structs directly.
- The stored offset is **not** the real offset: subtract
  `totalFileNameLength` to get the actual offset of the folder's
  file-record block.

## File-record block (per folder)

- Folder name as a `bzstring` (one length byte, then the name plus NUL;
  the length byte counts the NUL), present only if flag `0x1` is set.
- Then one 16-byte file record per file in the folder.

## File record (16 bytes)

- Name hash (u64).
- Size (u32). Bit 30 (`0x40000000`) flips the compression state relative
  to the archive default: if the archive is compressed by default, a
  file with bit 30 set is stored uncompressed, and vice versa. The
  stored size excludes the flag bit (`size & 0x3FFFFFFF`). For a
  compressed file the stored size covers the 4-byte original-size
  prefix plus the compressed payload.
- Offset (u32), absolute from the start of the file.

## File-name block

Present only if flag `0x2` is set. Root file names only (no paths),
NUL-terminated, in the same order as the file records across all
folders. Full paths are folder name + file name.

## File data blocks

- If flag `0x100` is set, the block starts with an embedded `bstring`
  (length byte counting the NUL, then the full path plus NUL).
- Compressed block: original size (u32), then the compressed payload
  (zlib for v104, LZ4 frame for v105).
- Uncompressed block: raw bytes.
- The size field covers the whole block, including any embedded name
  and the original-size prefix — confirmed against the real
  Textures0.bsa (2026-11-13): 5,889/5,889 files extract cleanly.

## Ordering

Folders, and files within each folder, are sorted by their 64-bit hash
(least to greatest). The reader does not depend on hash order; it reads
records as stored.

## Strings

- `bstring`: 1-byte length (counting the trailing NUL) + bytes + NUL.
- `bzstring`: same shape, used for folder names in file-record blocks.
- `zstring`: bytes + NUL, used in the file-name block.
