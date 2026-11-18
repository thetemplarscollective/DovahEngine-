# LAND — exterior terrain (Skyrim SE)

Exterior cells (`CELL` records with no interior flag) have a `LAND`
child record holding the terrain heightmap, normals, and vertex
colors for one 4096x4096-unit cell. This document records the
format as **proven against the real Skyrim.esm** (all 11,186
Tamriel LAND records), not as the older docs describe it — the
older docs are wrong about VHGT in Skyrim SE.

Validation: `crates/dovah-app/examples/vhgt_sweep.rs` (96
layout/orientation hypotheses scored by shared-edge continuity)
and `vhgt_offset.rs` (the DATA-offset question, scored over all
731,247 shared edge vertices in Tamriel). The winning decode
matches every shared edge vertex exactly (worst mismatch 0.0
across the 25-cell Riverwood test area; 99.96% of Tamriel's
shared edge vertices match within 0.5 units — the 290 outliers,
worst 12.0, are genuine seams in Bethesda's data).

## VHGT — heightmap (1096 bytes)

Layout: **float32 first height** (bytes 0-3, the southwest corner
height in Skyrim units) + **33x33 int8 deltas** (bytes 4-1092) +
**3 trailing bytes** (1093-1095, always 0 in the sampled cells —
padding).

Heights accumulate **mixed**:

- The first column accumulates **down**: each row's first vertex
  is the previous row's first vertex plus its delta
  (`h[y][0] = h[y-1][0] + D[y*33]`, with `h[0][0] = first +
  D[0]`).
- Each row accumulates **across** from its first vertex
  (`h[y][x] = h[y][x-1] + D[y*33+x]`).

`D[0]` (VHGT byte 4) is always 0 in the real data (verified
across all 11,186 Tamriel LANDs), so `h[0][0]` equals the first
height exactly.

This is **not** a plain sequential cumulative sum over all 1089
deltas: that reading makes neighbouring cells' shared edges
mismatch by thousands of units (the error is one row's total rise
per row). It is also **not** absolute int8 values (edges mismatch
by up to 144) and **not** the 4368-byte float grid of Morrowind /
Oblivion (Skyrim SE VHGT is 1096 bytes, not 4368).

Grid orientation: row 0 is the cell's **south** edge, column 0
the **west** edge. Vertex spacing is 128 units (32 intervals of
128 = the 4096-unit cell). No flips, no transpose.

## DATA — NOT a height offset (4 bytes)

The LAND `DATA` field holds values 25, 27, 29, or 31 across
Tamriel (31 in the Riverwood area). It is **not** added to the
heights: adding it breaks edge continuity (57,049 of Tamriel's
shared edge vertices off by more than 0.5, vs 290 without it).
Its exact meaning (flags / LOD level?) is unknown; the decoder
ignores it.

## Height scale — ×8, SETTLED (M13)

The accumulated VHGT height — **anchor included** — is
multiplied by 8 to get game units (`HEIGHT_SCALE` in
`land.rs`). UESP ("The final vertex Z, in game units relative
to the cell origin, is the cumulative result multiplied by 8")
and OpenMW (`Land::sHeightScale = 8.0f`) agree, and it is
proven directly against Dylan's Skyrim.esm: terrain vs 1,298
Riverwood REFRs gives median error −0.03 m under ×8 vs
+0.95 m under ×1 (60% within 0.5 m vs 17%), and mountain
trees are exact to centimetres under ×8 while floating ~31 m
under ×1 (see `docs/mod-tools-reference.md` §1 and the
exterior audit's Part F, which re-proves it with the shipped
decode).

Why this was never caught: a uniform scale error is INVISIBLE
to edge-continuity checks — scaling both sides of a shared edge
by the same factor keeps them matching. The `vhgt_sweep` proof
(edges match exactly) proves the accumulation and orientation,
but says nothing about scale.

Why ×1 produced BOTH of Dylan's symptoms (2026-11-14: "the
ground's to the roof"): under ×1 the terrain is 8× too flat AND
its absolute height is wrong. Where true heights are negative
(the village floor), ×1 terrain sits HIGHER than the true
ground, burying building bases by ~1 m; where true heights are
large positive (the mountain), ×1 terrain is far BELOW the
objects. One scale error explains both.

The one point that favoured ×1 — the inn door XTEL arrival
matching no-×8 terrain to 0.06 m (M11) — is explained, not
contradictory: the arrival sits at door-threshold height on the
inn's raised entrance (a porch ~1.3 m above the true terrain),
not at ground height. Under ×8 the arrival reads +1.35 m above
the ground, which is correct.

## VNML — normals (3267 bytes)

33x33x3 int8 normals, row-major in the same order as the heights:
one (x, y, z) triple per vertex, Skyrim space. Values are signed
bytes in the -127..127 range (divide by 127 for unit normals).

## VCLR — vertex colors (3267 bytes)

33x33x3 uint8 colors, row-major, one (R, G, B) triple per vertex.
This is a TINT over the VTXT texture layers, not the ground's
color itself: where the artist painted no tint the vertices are
pure white (255,255,255) and the texture shows at full strength.
Proven by `vclr_check.rs` (M10): the Riverwood 3x3 means are
RGB 213–239, most vertices exactly (255,255,255), with a few
painted darker (150s) for shading — so VCLR-only rendering
reads as pale white ground. The dirt/grass look needs VTXT;
do not "fix" the pale ground by darkening VCLR.

## VTXT — texture assignments (variable)

One entry per vertex quadrant, assigning ground textures. Not
decoded yet (M9 renders VCLR colors only).

## Related layouts (corrected against the real data)

- **CELL `XCLC` is 12 bytes**, not 8: int32 grid X, int32 grid Y,
  int32 unknown (0x0053fd00 for most Tamriel cells, 0x1700 for
  the persistent cell). All 16,978 exterior CELLs carry one.
  XCLC grid numbers are ambiguous across worldspaces (Sovngarde
  reuses Tamriel's grid), so an exterior loader must walk
  Tamriel's WorldChildren group (label = WRLD form 0x0000003c)
  rather than scanning all CELLs by XCLC.
- **REFR `XTEL` is 32 bytes**, not 28: uint32 destination door
  ref, float[3] arrival position, float[3] arrival rotation,
  uint32 tail (0 in every door checked). The arrival facing is
  the rot Z yaw: yaw 0 faces north (+Y) and yaw increases
  CLOCKWISE seen from above, so forward = (sin yaw, cos yaw).
  The counterclockwise reading was wrong — the inn door's XTEL
  yaw (-0.842) faces north-west out to the Riverwood road; the
  old formula faced north-east into the inn's west wall (M9
  screenshot, `spawn_viewpoint_faces_out_of_the_inn_door`).
- **Persistent refs** (teleport doors, quest markers) are filed
  under Tamriel's persistent cell (form 0x00000d74, grid (0,0)),
  not under the cell where they stand. An exterior loader must
  sweep the persistent cell by position: 139 persistent refs fall
  inside the Riverwood 3x3, including the inn's exterior door.
  Find that cell by FORM ID, never by grid (0,0): the geographic
  cell BleakwindBasinExterior01 (0x000099a2) shares grid (0,0),
  so a grid map holds only one of them — whichever it visits
  last. M9 first shipped the grid lookup and loaded the exterior
  with 0 doors; `persist_check.rs` proves the collision (2 CELLs
  at (0,0); 0x00000d74 has 15,196 children, 0x000099a2 has 71).
