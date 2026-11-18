# ESM/ESP — master and plugin files

Source: UESP wiki, "Skyrim Mod:Mod File Format"
(`https://en.uesp.net/wiki/Skyrim_Mod:Mod_File_Format`) and its TES4
subpage (`https://en.uesp.net/wiki/Skyrim_Mod:Mod_File_Format/TES4`).
Notes below are our own summary of those public specs.

## What it is

The database of the game world. A plugin file is a collection of
records — one per object (a creature, a weapon, a cell, a quest) —
organized into groups, with each record's details carried by fields.
`Skyrim.esm` is the master file; mods ship as `.esp` plugins, or `.esl`
light plugins, on top of it. All numbers are little-endian.

## Layout

```
TES4 record (the header, exactly one)
Top groups (GRUPs, one per record type, in a fixed order)
```

Skyrim.esm's top groups run in this order: GMST, KYWD, LCRT, AACT,
TXST, GLOB, CLAS, FACT, HDPT, HAIR, EYES, RACE, SOUN, ASPC, MGEF,
SCPT, LTEX, ENCH, SPEL, SCRL, ACTI, TACT, ARMO, BOOK, CONT, DOOR,
INGR, LIGH, MISC, APPA, STAT, SCOL, MSTT, PWAT, GRAS, TREE, CLDC,
FLOR, FURN, WEAP, AMMO, NPC_, LVLN, KEYM, ALCH, IDLM, COBJ, PROJ,
HAZD, SLGM, LVLI, WTHR, CLMT, SPGD, RFCT, REGN, NAVI, CELL, WRLD,
DIAL, QUST, IDLE, PACK, CSTY, LSCR, LVSP, ANIO, WATR, EFSH, EXPL,
DEBR, IMGS, IMAD, FLST, PERK, BPTD, ADDN, AVIF, CAMS, CPTH, VTYP,
MATT, IPCT, IPDS, ARMA, ECZN, LCTN, MESG, RGDL, DOBJ, LGTM, MUSC,
FSTP, FSTS, SMBN, SMQN, SMEN, DLBR, MUST, DLVW, WOOP, SHOU, EQUP,
RELA, SCEN, ASTP, OTFT, ARTO, MATO, MOVT, *HAZD*, SNDR, DUAL, SNCT,
SOPM, COLL, CLFM, REVB. Whether the engine requires this order is
unknown; the reader does not depend on it.

Six record types have a GRUP but no records: CLDC, HAIR, RGDL, SCPT,
SCOL, PWAT. There are two HAZD groups; the second holds no records.

## Plugin types

- **ESM** (`.esm`) — master files, pinned to the top of the load order.
- **ESP** (`.esp`) — standard plugins, loaded after masters.
- **ESL** (`.esl`) — light masters, introduced in Special Edition 1.5
  (Creation Club). Up to 4096 ESL plugins can load at once, but each
  holds at most 4096 form ID records. ESM and ESP files can also be
  flagged as ESL (TES4 flag `0x200`) if they fit the limit.

The TES4 record flags decide behavior regardless of file extension:
`0x1` = treated as a master, `0x200` = treated as a light master.

## TES4 — the header record

The first record in the file. Form ID is 0. Its fields:

| Field | Type | Notes |
| --- | --- | --- |
| HEDR | 12-byte struct | float32 version (1.7 for Skyrim), uint32 record+group count (excluding TES4 itself), uint32 next available object ID |
| CNAM | zstring | Author (max 512 bytes incl. terminator) |
| SNAM | zstring | Description (max 512 bytes incl. terminator) |
| MAST | zstring | Master filename; one MAST/DATA pair per master, in load order at save time. Form ID mod indices match this order |
| DATA | uint64 | Always 0; vestigial (master file size in older games) |
| ONAM | formid[] | Overridden cell children, only in ESM-flagged files that override their masters' cell children (ACHR, LAND, NAVM, PGRE, PHZD, REFR) |
| INTV | uint32 | Count of strings that can be tagified (Creation Kit option) |
| INCC | uint32 | Counter related to masters; introduced in the 1.6 update |

Skyrim.esm's HEDR reads version 1.7, 920,181 records and groups, next
object ID `0xFF000F93` — a useful check value when verifying against
the real file.

## Record (24-byte header + data)

| Field | Size | Notes |
| --- | --- | --- |
| Type | char[4] | e.g. `WEAP`, `NPC_`, `CELL`, `WRLD` |
| Data size | u32 | Size of the data that follows; does *not* include the header |
| Flags | u32 | See below |
| Form ID | u32 | See "Form IDs" below |
| Timestamp | u16 | Day/month/year encoding (SE uses `0bYYYYYYYMMMMDDDDD`) |
| Version Control Info | u16 | User IDs for check-out tracking |
| Internal version | u16 | Skyrim LE = 43, Skyrim SE = 44 |
| Unknown | u16 | Values 0–15 |

Record flags that matter to the reader:

| Flag | Meaning |
| --- | --- |
| `0x00000001` | Master (ESM) file (TES4 only) |
| `0x00000020` | Deleted record |
| `0x00000080` | Localized (TES4 only) — load the `.STRINGS` / `.DLSTRINGS` / `.ILSTRINGS` tables; without it, lstrings are treated as zstrings |
| `0x00000200` | Light master (TES4 only) |
| `0x00040000` | Data is compressed (see below) |

Other flags are record-type specific (quest item, persistent reference,
initially disabled, ignored, visible when distant, marker, navmesh
generation hints, ...). The reader stores flags verbatim and does not
interpret them yet.

### Compressed records

If flag `0x00040000` is set, the record data is: uint32 decompressed
size, then the fields compressed with **zlib**. Decompress first, then
parse fields from the result exactly as for an uncompressed record.

## Group — GRUP (24-byte header + children)

| Field | Size | Notes |
| --- | --- | --- |
| Type | char[4] | Always `"GRUP"` |
| Size | u32 | Size of the *entire* group, **including** the 24-byte header — unlike records and fields, whose sizes exclude their headers |
| Label | 4 bytes | Format depends on group type (below). Not fully reliable: the Creation Kit's "ignore group" marking corrupts it |
| Group type | i32 | See below |
| Timestamp | u16 | As in records |
| Version Control Info | u16 | As in records |
| Unknown | u32 | Varies by group type; often 0 or `0xCCCCCCCC` |
| Children | size − 24 bytes | Records and subgroups |

Group types:

| Type | Name | Label holds |
| --- | --- | --- |
| 0 | Top | Record type, char[4] (e.g. `GMST`) |
| 1 | World Children | Form ID of the parent WRLD |
| 2 | Interior Cell Block | Block number (i32) |
| 3 | Interior Cell Sub-Block | Sub-block number (i32) |
| 4 | Exterior Cell Block | Grid Y, X (i16[2], note reverse order) |
| 5 | Exterior Cell Sub-Block | Grid Y, X (i16[2], note reverse order) |
| 6 | Cell Children | Form ID of the parent CELL |
| 7 | Topic Children | Form ID of the parent DIAL |
| 8 | Cell Persistent Children | Form ID of the parent CELL |
| 9 | Cell Temporary Children | Form ID of the parent CELL |

Groups nest: a top group holds records, and the CELL, WRLD, and DIAL
top groups also hold child groups after their main records. The child
groups carry structural meaning — e.g. a CELL's references split into
persistent and temporary children, and world children hold the cells
belonging to a worldspace. Record types found only in subgroups:
REFR, ACHR, NAVM, PGRE, PHZD, LAND, INFO.

## Field — subrecord (6-byte header + data)

| Field | Size | Notes |
| --- | --- | --- |
| Type | char[4] | e.g. `EDID`, `FULL`, `DATA`, `OBND`, `VMAD` |
| Size | u16 | Size of the data that follows |
| Data | size bytes | Format depends on record and field type |

### XXXX — oversized fields

A field's size is a u16, so it caps at 65,535 bytes. For larger
payloads (commonly navmesh fields), the preceding field has type
`XXXX`, a u16 size of 0, and a u32 payload holding the real size of
the *following* field. The reader must apply that size to the next
field and not treat XXXX itself as a real field.

### Common fields

`EDID` (editor ID, zstring) identifies most records by name and is the
reader's one parsed string field. `OBND` (object bounds) appears on
many records and stays raw bytes for now. `VMAD` (scripting) is
parsed by the `vmad` module — see `vmad.md` for the full layout.
Other per-record field semantics are a later milestone.

## Form IDs

A 32-bit unique ID per form. The top byte is the mod index — the
file's position in the load order (matching the MAST list order in
TES4): `00` is Skyrim.esm, `01` Update.esm, `02` Dawnguard, `03`
Hearthfires, `04` Dragonborn in Special Edition. `FF` marks
dynamically allocated IDs (save-game specific, never documented).
`FE` marks light-plugin (ESL/Creation Club) IDs, where the next three
hex digits are the plugin's position in the ESL load order and the
last three digits are the object ID. The low three bytes (or low six
hex digits for ESL) are the object ID within that file.

Base IDs identify a prototype (e.g. an iron sword type); reference
IDs identify a specific placed copy in the world.

## Strings

- `zstring`: bytes plus NUL terminator (EDID, CNAM, MAST, ...).
- `lstring`: a uint32 index into the external string tables, used only
  when the TES4 localized flag (`0x80`) is set; otherwise treated as a
  zstring. String-table lookup is a later milestone.

## Reader notes (dovah-esm)

- The M3 parser reads the whole plugin into memory (`Plugin::from_bytes`
  / `Plugin::open`). Skyrim.esm is ~250 MB, which is fine on the build
  VM, but a lazy, seek-based index (like dovah-bsa) is future work for
  the renderer phase — the phone will not want the whole ESM resident.
- Records are stored with raw field payloads; only EDID (as a string)
  and the TES4 HEDR struct are interpreted.
- Compressed records are decompressed with zlib at parse time.
- XXXX oversized fields are handled.
- The parser is strict at the top level: the first record must be
  TES4, and everything after it must be a GRUP. Unknown group types
  parse as `GroupType::Unknown` rather than failing.
