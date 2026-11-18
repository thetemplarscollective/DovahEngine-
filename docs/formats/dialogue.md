# Dialogue (DIAL / INFO) and localized strings — proven formats (M18)

Every fact here is proven from Dylan's real `Skyrim.esm` and
`Skyrim_-_Patch.bsa` by the `dialogue_probe*` examples in
`crates/dovah-app/examples/` (kept as the durable research
record) and pinned by tests (`dovah-esm/tests/dialogue_real.rs`,
`dovah-strings/tests/real_files.rs`). Counts are the real
file's counts.

## .strings files

One file set per plugin (`Skyrim_English.*`,
`Update_English.*`, `Dawnguard_English.*`,
`HearthFires_English.*`, `Dragonborn_English.*`), three kinds
per set. The English sets live inside `Skyrim_-_Patch.bsa` at
`strings/<stem>_english.strings` / `.dlstrings` / `.ilstrings`
(lowercase, forward slashes); the same 15 files are also in
`test-data/strings/Data/Strings/` (Nexus mod 1842 — byte
identical to Patch.bsa, same counts). The runtime reads them
from Patch.bsa via the BSA provider — no separate Strings
folder on the phone.

| Kind | Holds | Entry layout |
| ---- | ----- | ------------ |
| `.STRINGS` | General strings: names (a record's `FULL`), topic text, UI labels | Plain NUL-terminated text, NO length prefix |
| `.ILSTRINGS` | **Dialogue lines (INFO `NAM1`)** + interface strings | u32 length prefix (INCLUDING the NUL), then text |
| `.DLSTRINGS` | Long-form text (book text with font markup, loading tips, descriptions) | u32 length prefix (INCLUDING the NUL), then text |

**NAM1 -> `.ILSTRINGS`, NOT `.DLSTRINGS`.** The remembered
docs that say dialogue lines live in DLSTRINGS are WRONG for
Skyrim SE: all 11 NAM1s tested resolved in ILSTRINGS and NONE
in DLSTRINGS (Nexus 1842 and Patch.bsa agree). Do not "fix"
this back.

Header: `u32 count` + `u32 data_size`, then `count` x
`(u32 id, u32 offset)` directory (sorted by id), then the
string data. The directory offset is relative to the data
section start (byte `8 + count*8`). In `.DLSTRINGS` /
`.ILSTRINGS` the text starts 4 bytes past the offset (after
the length prefix) — the first DLSTRINGS entry reads
`18 01 00 00` (280) then 279 text bytes + NUL, and the next
entry sits exactly 284 bytes on. A reader ignoring the prefix
decodes garbage; a reader applying it to `.STRINGS` skips the
first four letters of every name.

String IDs are the directory IDs verbatim — the u32 in the
record IS the ID in the file. No masking, no high-byte file
marker. Which file to look in is decided by the FIELD, never
by the ID: `FULL` -> `.STRINGS`, INFO `NAM1` -> `.ILSTRINGS`.

**Same-plugin rule REQUIRED:** the same string ID means
different text in different plugins (NAM1 10739 is "There you
are." in Skyrim and "I think you may have wasted your money,
friend." in Dragonborn). Resolve in the set of the RECORD's
plugin (form ID top byte = load-order index: 0 Skyrim, 1
Update, 2 Dawnguard, 3 HearthFires, 4 Dragonborn).

Skyrim counts: STRINGS 30,301 / DLSTRINGS 2,686 / ILSTRINGS
34,427.

Known IDs (pinned by tests): FULL 378 -> "Delphine", 56228 ->
"Orgnar", 60445 -> "Embry", 1763 -> "Iron Sword". NAM1 62071
-> "If we could get into the Thalmor Embassy... it's the
centre of their operations in Skyrim." NAM1 63455 -> "I'm not
sure yet. I have a few ideas, but I'll need some time to pull
things together...."

## DIAL records (topics) — 15,037 in Skyrim.esm

| Field | Size | Meaning |
| ----- | ---- | ------- |
| `EDID` | zstring | Editor ID (6,945 have one) |
| `FULL` | 4 bytes | String ID -> `.STRINGS`: the topic text — the PLAYER's line (5,170 have one) |
| `SNAM` | 4 chars | Subtype code — **the classifier** (every DIAL has one) |
| `PNAM` | 4 bytes, f32 | Priority (50.0 / 80.0 / 40.0 seen) |
| `QNAM` | 4 bytes | Quest form ID |
| `BNAM` | 4 bytes | Branch topic form ID (sometimes) |
| `DATA` | 4 bytes | Correlates exactly with SNAM (see below) |
| `TIFC` | 4 bytes, =1 seen | Likely the INFO count |

Most DIALs have NO EDID and NO FULL — scene/forced topics,
not player-selectable.

SNAM subtypes (exact counts): `CUST` 6,503 (custom =
player-selectable topics), `SCEN` 7,426 (scene dialogue),
`HELO` 297 (hello/greeting), `GBYE` 129 (goodbye), `IDAT`
159, `HIT_` 59, `DETH` 52, `ATCK` 36, `TAUT` 34, `IDLE` 33,
`NOTC` 26, `ALTC` 22, `RUMO` 21, `NOTA` 15, `COTN` 14,
`TRES` 12, `PFGT` 11, `POAT` 11, `LOTC` 11, `ALTN` 10,
`COLO` 9, `ALIL` 8, `FLEE` 8, `BASH` 8, `LOTN` 8, `LOIL` 7,
`BLOC` 6, `NOTI` 6, `PCSH` 5, `MUNC` 4, `OBCO` 4, and
singletons/triples (`SHOW`, `MREF`, `REFU`, `FEXT`, `AGRE`,
`ZKEY`, `ACAC`, `DFDA`, `TITG`, `KNOO`, `VPSL`, `VPEL`,
`ASSA`, `WTCR`, `PURS`, `DEOB`, `MURD`, `FIWE`, `OUTB`,
`PICC`, `PICN`, `PICT`, `ASNC`, `STOF`, `STFN`, `PIRN`,
`ENBZ`, `ACYI`, `INTI`, `LOOB`, `VPES`, `GRNT`, `LWBS`,
`BRIB`, `EXBZ`, `TRAN`, `BREA`, `STEA`, `BLED`).

`DATA` (4 bytes) correlates exactly with SNAM: byte0 is a
flag (almost always 0; bit0 set on a few), byte1 is 0 for
player topics (CUST/RUMO), 2 for scene, 3 for combat barks,
7 for ambient (HELO/GBYE/IDLE); byte2 is a sub-index; byte3
is always 0. M18 uses SNAM, not DATA.

**Greetings are `SNAM == "HELO"`.** EDID matching does NOT
work for greetings (only 9 DIALs have "Greeting" in the EDID,
all quest branch CUST topics).

## INFO records (responses) — 31,465 under 14,904 topics

INFOs live in `TopicChildren` groups (group type 7, label =
parent DIAL form ID). 133 DIALs have no INFO children. File
order is the game's priority order (assumed; no explicit
priority field was found).

| Field | Size | Meaning |
| ----- | ---- | ------- |
| `NAM1` | 4 bytes | String ID -> `.ILSTRINGS`: the response — what the NPC says back |
| `TRDT` | 24 bytes, six u32s | Response header; field index 3 = response index within the INFO (1, 2, 3 seen); fields 0/1 may be emotion type/value — unverified |
| `NAM2` / `NAM3` | 1 byte each | Empty (seen) |
| `CTDA` | 32 bytes | One condition (see below) |
| `ENAM` | 4 bytes | Flags (?) |
| `CNAM` | 1 byte | |
| `VMAD` / `SCHR` / `QNAM` / `DNAM` / `NEXT` | various | Script hooks (script-only INFOs) |

One INFO can hold MULTIPLE (`TRDT`, `NAM1`, `NAM2`, `NAM3`)
response groups — alternative lines; the game picks one.
M18 takes the first resolvable NAM1 (documented
approximation). Some INFOs are script-only and have NO NAM1
(e.g. INFO `0x00016f8c`: VMAD + DATA + CNAM + DNAM + CTDA +
SCHR + QNAM) — callers must skip those (fall to the next
targeting INFO; a topic with no speakable response is dropped
from the list).

## CTDA conditions (32 bytes)

```text
0   u8    operator
1   u8[3] padding — NONZERO in some conditions (e.g.
        [0x4e, 0x32, 0x2e]), purpose unknown, ignored
4   f32   comparison value (1.0f seen)
8   u16   function — A u16, NOT a u32 (see the trap)
10  u16   unknown — nonzero sometimes (e.g. 0x18a7), ignored
12  u32   param1 — the NPC form ID for GetIsID
16  u32   param2
20  u32   run-on
24  u32   reference
28  u32   unknown (0xffffffff seen)
```

Function 72 (`0x48`) = `GetIsID`: the condition passes when
the speaker is the NPC in param1.

**THE TRAP:** reading the function as a u32 at offset 8 gives
garbage (e.g. `0x18a70048` = 413,597,768) whenever the
unknown u16 at offset 10 is nonzero — and that hid ~50
GetIsID conditions in the first probe round. Read it as a u16
at offset 8.

## Per-NPC results (inn, pinned by `dialogue_real.rs`)

| NPC | Form ID | DIALs with any targeting INFO | CUST topics with FULL | HELO greetings |
| --- | ------- | ----------------------------- | --------------------- | -------------- |
| Delphine | 0x00013478 | 171 | 120 (103 with a resolvable line, 17 script-only) | 6 (5 with lines) |
| Orgnar | 0x00013479 | 10 | 4 | 1 |
| Embry | 0x0003550b | 2 | 0 | 1 |

Embry's dialogue is genuinely minimal — greeting + goodbye
only. Honest, not a gap.

Delphine's first HELO in file order is the greeting M18 shows
(several of her greetings are late-quest lines — without
condition evaluation the file order is the only honest pick).

## What M18 does NOT do

Full condition evaluation (quest stages, factions, items,
etc.) is out of reach this milestone: M18 shows all topics
for the NPC (targeted by GetIsID) and says so plainly. No
gating is faked. No voice audio (Voices_en0.bsa is a later
milestone). No trading.
