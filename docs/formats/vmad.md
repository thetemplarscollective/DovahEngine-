# VMAD — script data on a record

Source: UESP wiki, "Skyrim Mod:Mod File Format/VMAD Field"
(`https://en.uesp.net/wiki/Skyrim_Mod:Mod_File_Format/VMAD_Field`).
Notes below are our own summary of that public spec, checked
against Dylan's real Skyrim.esm (see the verification note at the
end).

## What it is

A `VMAD` field holds the Papyrus script data attached to a record:
the names of the attached scripts, the initial values of their
properties (as set in the Creation Kit), and — for some record
types — script fragments. Any record that has scripts carries one:
items, dialogue topics, packages, quests, placed references, NPCs.

All numbers are **little-endian** (unlike PEX, which is
big-endian). Strings are `wstring`s: a u16 length followed by that
many Windows-1252 bytes, no terminator.

The field must be processed sequentially — the lengths and
locations of its sections are not given, so there is no way to skip
to a section. Every VMAD field has a Primary Scripts Section; most
records have nothing else. Records with fragments append a
Fragments Section whose layout depends on the record type.

## Primary Scripts Section

| Field | Type | Notes |
| --- | --- | --- |
| version | int16 | 5 most recent, 2 minimum |
| objFormat | int16 | 1 or 2; affects how object-type properties are read |
| scriptCount | uint16 | Number of scripts directly attached to this record |
| scripts | Script[scriptCount] | Below |
| fragments | Fragments | Below; absent when the field ends here |

### Script

| Field | Type | Notes |
| --- | --- | --- |
| scriptName | wstring | Name of the script, without extension |
| status | uint8 | Only present if version >= 4 (defaults to 0). 0 = local script, 1 = inherited with altered properties, 3 = inherited and then removed |
| propertyCount | uint16 | Number of properties defined in this script |
| properties | Property[propertyCount] | Below |

### Property

| Field | Type | Notes |
| --- | --- | --- |
| propertyName | wstring | Name of the property |
| propertyType | uint8 | Type code (below) |
| status | uint8 | Only present if version >= 4 (defaults to 1). 1 = property edited, 3 = property removed |
| data | depends on type | Below |

Property type codes:

| Code | Type | Data |
| --- | --- | --- |
| 1 | object | 8 bytes (below) |
| 2 | wstring | wstring |
| 3 | int | int32 |
| 4 | float | float32 |
| 5 | bool | int8 (nonzero = true) |
| 11 | array of objects | uint32 count, then that many objects |
| 12 | array of wstrings | uint32 count, then that many wstrings |
| 13 | array of ints | uint32 count, then that many int32s |
| 14 | array of floats | uint32 count, then that many float32s |
| 15 | array of bools | uint32 count, then that many int8s |

Array types are only supported when version >= 5.

An **object** value is always 8 bytes, but the byte order depends
on `objFormat`:

- objFormat 1: bytes 0–3 are a form ID (uint32), bytes 4–5 are the
  alias ID (uint16), bytes 6–7 are zero.
- objFormat 2: bytes 0–1 are zero, bytes 2–3 are the alias ID
  (uint16), bytes 4–7 are a form ID (uint32).

The alias ID is 0xFFFF (−1) whenever the form ID does not point to
a quest, in which case the form ID is assigned directly. Otherwise
the alias ID is the quest alias the property points at.

## Fragments Section

Only INFO, PACK, PERK, QUST, and SCEN records ever have fragments.
When a record has none, the VMAD field simply ends after the
Primary Scripts Section. Note the fragment count appears *before*
the file name in some layouts and *after* it in others.

### INFO / PACK

Fragment scripts live in a `TIF_` (INFO) or `PF_` (PACK) file.

| Field | Type | Notes |
| --- | --- | --- |
| unknown | int8 | Always 2 |
| flags | uint8 | Script locations: 0x1 = begin, 0x2 = end, 0x4 = change (PACK only) |
| fileName | wstring | Fragment script file name, without extension |
| fragments | Fragment[flagsCount] | One per set bit, in begin/end/change order |

Fragment: unknown int8, scriptName wstring, fragmentName wstring
(typically `"Fragment_5"`).

### PERK

Fragment scripts live in a `PRKF_` file.

| Field | Type | Notes |
| --- | --- | --- |
| unknown | int8 | Always 2 |
| fileName | wstring | Fragment script file name |
| fragmentCount | uint16 | |
| fragments | Fragment[fragmentCount] | |

PERK fragment: index uint16, unknown int16, unknown int8,
scriptName wstring, fragmentName wstring.

### QUST

Fragment scripts live in a `QF_` file. After the fragments come
the scripts attached to the quest's aliases.

| Field | Type | Notes |
| --- | --- | --- |
| unknown | int8 | Always 2 |
| fragmentCount | uint16 | |
| fileName | wstring | Fragment script file name |
| fragments | Fragment[fragmentCount] | |
| aliasCount | uint16 | Number of aliases with attached scripts |
| aliases | Alias[aliasCount] | |

QUST fragment: index uint16 (quest stage, the QUST `INDX`
value), unknown int16 (always 0), logEntry int32 (log entry
within the stage), unknown int8 (always 1), scriptName wstring,
fragmentName wstring.

Alias: object (8 bytes, same layout as a property object — the
form ID is the quest's own), version int16 (same as the primary
version), objFormat int16 (same as the primary objFormat),
scriptCount uint16, scripts (same layout as the Primary Scripts
Section).

### SCEN

Fragment scripts live in an `SF_` file. Begin/end fragments come
first, then phase fragments.

| Field | Type | Notes |
| --- | --- | --- |
| unknown | int8 | Always 2 |
| flags | uint8 | 0x1 = begin, 0x2 = end |
| fileName | wstring | Fragment script file name |
| beginEnd | Fragment[flagsCount] | Begin first when both are present |
| phaseCount | uint16 | |
| phases | PhaseFragment[phaseCount] | |

Phase fragment: unknown int8, phase uint32 (0-based in the data,
1-based in the CK), unknown int8, scriptName wstring,
fragmentName wstring.

## Reader notes (dovah-esm `vmad` module)

- `Vmad::parse(data, record_type)` parses the whole field and
  rejects trailing bytes, bad versions (outside 2–5), bad object
  formats (not 1 or 2), unknown property types, and array types
  before version 5. A fragments section on a record type that
  never has one is an error.
- `Record::vmad()` is the convenience entry: `None` when the
  record has no VMAD field, `Some(Err(..))` when it is malformed.
- The fragment marker byte (always 2) is validated wherever the
  spec says it is always 2.

## Verification against real data

Checked 2026-11-13 against Dylan's Skyrim.esm with
`cargo run --example vmad_check`: **16,133 records carry VMAD,
all parse cleanly, zero errors** — 17,407 attached scripts,
46,493 properties (39,644 object, 3,210 bool, 2,846 int, 613
float, 178 string, 2 array-of-bool), versions 4 (2,561) and 5
(13,572), object formats 1 (2,705) and 2 (13,428). Fragments:
5,570 INFO/PACK records, 5 PERK, 856 QUST, 557 SCEN — 12,355
fragment scripts and 2,529 quest-alias scripts. The integration
test `tests/vmad_real.rs` pins these counts.
