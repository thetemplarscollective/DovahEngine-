# Mudcrab reference — what the Mudcrab engine proves about Skyrim's formats and systems

Research done 2026-11-15 (read-only: no code edited, no builds run, no
cargo). Method: read the Mudcrab source and docs in
`references/mudcrab/` (depth-1 clone of
https://github.com/Mudcrab-Team/mudcrab, GPL-3.0, Rust + Bevy), then
checked each claim that matters against our own tree. No Mudcrab code is
quoted here beyond short identifiers; mechanisms are described in our own
words with file + line citations.

**Architecture warning, read first.** Mudcrab is convert-first: plugins
→ SQLite world DB, meshes → glTF 2.0, textures → KTX2, Papyrus → Luau.
We read the ORIGINAL formats live (BSA/ESM/NIF/DDS). Their FORMAT
findings transfer directly; their PIPELINE findings transfer only as
design ideas (what data to extract, what the runtime needs) — never as
code to copy, and never as a reason to add a conversion step. Where a
finding does not apply because of this difference, the finding says so.

Each finding gives: the fact, its source (file + line, or doc path),
what our code does today, and what should change (or confirmation our
code is already right).

**Headline results**

1. **Collision is fully mapped from the NIFs.** Havok collision lives in
   `bhkCollisionObject` trees inside the NIFs: every shape family, the
   rigid-body layout, the layer table, and the player-controller numbers
   are all decoded (§1–§5). This is the blueprint for our collision
   milestone — including the stairs/porches Dylan is walking through.
2. **Door crossing is symmetric in their implementation.** The return
   trip needs no special case: fade → switch at full black → hold black
   until the landing cell is resident and settled → restore the player
   on failure (§7). That state machine is the checklist for diagnosing
   our broken exterior→inn return.
3. **Walking NPCs need pose-only clips + speed×dt — no root motion.**
   The `.hkx` format is solved (retail SE uses only THREECOMP40
   rotation quantisation), and their engine drives gait from speed
   alone (§9–§10).
4. **Our terrain textures tile 3× too coarsely.** The game tiles land
   textures 24× per cell; our VTXT bake tiles 8× (`TILE_QUADS = 4.0`).
   One constant to verify against a render (§19).
5. **The mist needs the palette-lookup effect shader.** `newFog` is a
   `BSEffectShaderProperty` with Greyscale_To_PaletteColor/Alpha: the
   grey level picks a colour AND an alpha from a palette texture. Our
   gradient-multiply is the wrong mechanism (§15).
6. **We don't read XRDS.** 10,810 of 12,148 `LIGH` references carry a
   per-reference radius override that wins over the record's radius
   (§22).

---

## 1. Collision: Havok data lives in the NIFs — the full shape decode

### The fact
Every collidable object carries its collision in its NIF as a
`bhkCollisionObject` → `bhkRigidBody`/`bhkRigidBodyT` → shape tree.
Havok values in NIFs are in METRES (`HAVOK_TO_CREATION = 70.0`).
The converter decodes these shape families (`extract_shape`):

- `bhkMoppBvTreeShape` — unwrap to its child (the MOPP tree itself is
  not decoded; the child shape is the geometry).
- `bhkTransformShape` / `bhkConvexTransformShape` — 4×4 matrix at
  offset 20.
- `bhkListShape` — a list of child shapes.
- `bhkCompressedMeshShape` (+ `bhkCompressedMeshShapeData`) — requires
  bits == 17, winding_bits == 18. **This is the shape Riverwood's
  bridge and walkway stairs use (§5).**
- `bhkNiTriStripsShape` — triangle strips.
- `bhkCapsuleShape` — endpoints at offsets 16/32, radius max(@28, @44).
- `bhkSphereShape` — material @0, radius @4 → a zero-length capsule.
- `bhkCylinderShape` — radius @48 → a 16-point × 2-ring hull.
- `bhkMultiSphereShape` — count @16, then `NiBound` {centre, radius}
  × n from offset 20, 1–8 spheres.
- `bhkBoxShape` — half-extents @16 → an 8-corner hull.
- `bhkConvexVerticesShape` — count @32, vertices from @36, 16-byte
  stride → a hull.

Output is one of four engine shapes: `CollisionShape::{Mesh, Capsule,
Box, Hull}`. Limits: MAX_VERTICES 1M, MAX_SHAPES 256, nesting depth
< 16.

The rigid-body layout (Skyrim SE, BS v100) is: `bhkWorldObject` shape
@0, filter @4, info 8..28; `bhkRigidBodyCInfo2010` from 32; inertia
@116 (hkMatrix3, 3 rows × 4 floats); centre of mass @164 (Vector4);
mass @180; linear damping @184; angular damping @188; friction @200;
restitution @208; max linear vel @212; max angular vel @216; motion
system @224; deactivator @225; quality @227; min length 228.
Motion systems: 0 invalid, 1 dynamic, 2 sphere inertia, 3 sphere
stabilized, 4 box inertia, 5 box stabilized, 6 keyframed, 7 fixed,
8 thin box, 9 character; simulated = [1,2,3,4,5,8]. Quality: 0
invalid, 1 fixed, 2 keyframed, 3 debris, 4 moving, 5 critical,
6 bullet, 7 user, 8 character, 9 keyframed report; moving =
[3,4,5,6]. `classify`: Dynamic iff simulated motion + moving quality
+ mass > 0; Keyframed iff motion 6 + mass 0; else Fixed.

Havok layers (from nif.xml): 0 UNIDENTIFIED, 1 STATIC, 2 ANIMSTATIC,
3 TRANSPARENT, 4 CLUTTER, 5 WEAPON, 9 TREES, 10 PROPS, 12 TRIGGER
(not physical — bails), 13 TERRAIN, 15 NONCOLLIDABLE (passable,
skipped), 17 GROUND, 26 TRANSPARENT_SMALL, 27 INVISIBLE_WALL, 28
TRANSPARENT_SMALL_ANIM, 31 STAIRHELPER, 35 COLLISIONBOX.

### Sources
- `crates/converter/src/collision.rs` (2,029 lines): rigid-body
  layout :144-178, `extract_shape` :407+, `HAVOK_TO_CREATION`.
- `crates/shared/src/collision.rs:73` `CollisionShape`.

### What our code does today
Nothing: we have no per-object collision at all — only a floor grid
+ room bounds. `dovah-nif` does not parse any `bhk*` block.

### What should change (collision milestone)
This is the map. Parse `bhkCollisionObject`/`bhkRigidBody` + the
shape families in `dovah-nif` (live, at load — no conversion step),
and build Rapier colliders from them the way §4 describes. The
compressed-mesh shape is the critical one for Riverwood (stairs,
bridge). Layers 12 and 15 are the two skip cases; 31 STAIRHELPER is
physical (§5).

---

## 2. Player controller: kinematic capsule + custom step logic

### The fact
The player is a `RigidBody::KinematicPositionBased` with a
`Collider::capsule_y(half_height, radius)` and Rapier's
`KinematicCharacterController` (slide true, `autostep: None` — they
do stepping THEMSELVES, max slope climb 50°, min slope slide 55°,
snap_to_ground 12, offset 2.0, `apply_impulse_to_dynamic_bodies:
false` — upstream Rapier 0.35 has a manifold-transfer panic otherwise).
Collision groups: GROUP_WORLD=1, GROUP_PLAYER=2, GROUP_TANKARD=3,
GROUP_CLUTTER=4.

Stepping is custom (`walk_step_rise`): bounded capsule sweeps via
`cast_shape` (target_distance 2.0, dynamic bodies excluded from
stepping). It handles: skin-gap repair (lift ≤ autostep, only the
measured overlap), wall hit → corner probe (`corner_advance = radius
× (1 − sin(max_slope))`), overhead clearance sweep (up 36), forward
sweep at the raised height, downward landing cast (the landing normal
must be walkable), rise = landing.witness1.y + standing_height/2 +
offset − body.y; lift = (rise + 0.05).min(budget). It rejects:
ceiling hits, glancing contacts beyond capsule reach, rise ≤ 0.05 or
> budget.

Stair tests prove the envelope: Bleak Falls Tower stairs are ≈
24-unit rises/treads; a 16-step flight × 24 = 384 rise is climbed at
speeds 80.1 and 370.0, angles 0°/±20°/45°, seams 0/0.25. A 48-unit
rise is BLOCKED (above autostep 36). A ceiling at 120 blocks a
24-unit step (100.8 + 24 = 124.8 > 120). An unsupported capsule must
not mantle. Skin recovery is ≤ 1 skin width; deep penetration is not
lifted.

### Sources
- `crates/engine/src/physics.rs:1652-1676` (controller spawn),
  `:1801-1910` (`walk_step_rise`).
- `crates/engine/src/physics/stair_tests.rs` (envelope tests; the
  packed test is ignored, needs `MUDCRAB_STAIR_ASSETS` and real
  converted collision from GLB extras `mudcrabCollision`).

### What our code does today
No character controller — the player is a camera with a floor grid.
No stairs, no stepping, no capsule.

### What should change (collision milestone)
Adopt the shape, not the code: kinematic capsule + Rapier
character controller with `autostep: None` + custom step logic on
`cast_shape`. The numbers in §3 are the starting tuning. The stair
envelope (24u rises climb, 48u blocked, ceiling check) is the test
suite to copy — Riverwood's porches and walkway stairs are the same
problem at the same scale.

---

## 3. Movement numbers: capsule, speeds, gravity, jump

### The fact
`MovementTuning` defaults: walk 160, run 300, sprint ×1.5,
horizontal accel 1800, gravity 900 u/s² (`GRAVITY_CREATION_UNITS`),
jump launch 340, capsule radius 28, capsule standing height 100.8,
eye height 89.6, slope climb 50°, slope slide 55°, autostep height
36, ground snap 12. `PHYSICS_TIMESTEP` 1/60 s; `MAX_LINEAR_SPEED`
20,000; `MAX_LIVE_DYNAMIC_CLUTTER` 256; `PLAYER_CONTROLLER_OFFSET`
2.0. Speeds can come from the player's MOVT record (their converter
reads a `movement_types` table; the player NPC_ is 0x00000007, race
via RNAM).

MOVT SPED is 11 LE f32s: Left walk 80.09 / run 370.00, Right 79.75 /
370.00, Forward 80.10 / 370.00, Back 71.93 / 205.25; the last 3
floats are 3.14159 (rotation values). GMST `fMoveCharWalkBase`
0x0001EC72 = 100.0; `fJumpHeightMin` 0x000ABEF6 = 76.0. NO gravity
GMST exists in the five masters. Jump apex with their numbers:
340²/(2×900) = 64.22 units. NPC_Default_MT MOVT is 0x0003580D;
NordRace has no WKMV/RNMV — movement-type selection is a project
rule, not a record link.

### Sources
- `crates/engine/src/physics.rs:88-101` (`MovementTuning`).
- `docs/player-movement-record-map.md` (MOVT map, GMSTs).
- `docs/player-sprint-research.md` (§24 — the sprint multiplier is
  provisional, not proven).

### What our code does today
`EYE_HEIGHT = 1.7` (119 units — taller than their 89.6). No gravity,
no jump, no capsule, no MOVT reading. Our eye height vs theirs is a
1.33× difference worth checking against the real game's camera:
theirs is the Creation-unit value from the controller; ours was set
by feel.

### What should change (collision milestone)
Take the capsule (28r / 100.8h), gravity 900, jump 340, autostep 36,
slopes 50°/55° as the starting tuning. Reconcile eye height (89.6u
= 1.28 m vs our 1.7 m) — measure against a render with a known
reference (door height, NPC eye level) before changing. Read MOVT
SPED for walk/run speeds in a later pass; hardcode 160/300 first.

---

## 4. Engine collider build: authored first, proxy second, tiers by family

### The fact
Authored collision becomes Rapier colliders: Mesh →
`Collider::trimesh` (limits 1M verts / 2M tris); Capsule →
`Collider::capsule`; Hull → `Collider::convex_hull`. The terrain
collider is a trimesh from the EXACT rendered terrain mesh buffers
— never a separately computed heightfield, so the walkable ground
and the visible ground cannot disagree.

When authored collision is ABSENT, a static proxy fallback builds a
trimesh from render triangles — but only for eligible families, and
only for older GLBs; authored absence NEVER falls back (a deliberate
rule: if the author said "no collision", there is none). Tiers:
Full = STAT/TREE/FURN (authored + proxy); AuthoredOnly = ACTI
(fixed/keyframed bodies only, no proxy); VerifiedContainer = CONT,
exactly 4 audited paths (barrel01, tgbarrel01, sovbarrel01,
sburialurn01) requiring version-2 body ownership + one massless
fixed STATIC-layer root body on node 0 owning every shape.

Dynamic clutter (MISC/WEAP/ARMO/BOOK/AMMO/ALCH/INGR/SLGM/KEYM/SCRL
with a dynamic body): dynamic Rapier body, authored mass properties,
non-convex → one hull, CCD, first body only, cap 256 live, bodies
start asleep. CONT/MSTT/ACTI/FLOR/DOOR/FURN are excluded from the
dynamic path.

### Sources
- `crates/engine/src/streaming.rs:2463+` (collider build), `:2046`
  (`terrain_collider_from_mesh`), `:2103` (`fixed_collision_tier`).

### What our code does today
No colliders of any kind. Our terrain mesh is built in `land.rs`
(`terrain_mesh`); a trimesh from those same buffers is the direct
equivalent of their terrain collider.

### What should change (collision milestone)
Build the terrain trimesh from the rendered buffers first (one
change, fixes ground collision everywhere), then authored NIF
collision for STAT/TREE/FURN, then the proxy fallback for
authored-absent meshes. The "authored absence never falls back"
rule is worth adopting: it keeps invisible walls from appearing
where the game deliberately has none. Dynamic clutter is later
work (pickup physics); the 256 cap is a good default when we get
there.

---

## 5. Riverwood's stairs ARE authored — the exact case we lack

### The fact
Riverwood's bridge + walkway stairs have authored COMPRESSED
collision meshes. The stair mesh separates WOOD vs STAIRS_WOOD
material but both stay physical (the public NIF schema does not
prove Skyrim's controller ignores either). STAIRHELPER is accepted
as physical. Live stair traversal is a manual gate (Rapier autostep
needs clearance + landing width). The mill log pile is a separate
FURN `MillLogPile.nif` with authored collision. The lumbermill
walkable ramp/boards are `woodwalkway01` with MASK alpha; RockCliff
BLEND faces get a proxy exception. Runtime metrics distinguish
authored / partial / absent / proxy / skipped.

### Sources
- `docs/riverwood-static-collision.md` (107 lines).
- `crates/engine/src/streaming.rs` `static_proxy_material_allowed`.

### What our code does today
Dylan walks under/through the porches and stairs (confirmed on the
phone 2026-11-15, v0.4.6). No collision exists for any of it.

### What should change (collision milestone)
This is the acceptance test for the milestone: the bridge, the
walkway stairs, and the mill ramp must be walkable, and the porch
roofs must block. The compressed-mesh shape (§1) is the one shape
family the milestone cannot ship without. Their material note
(WOOD vs STAIRS_WOOD both physical) removes a rabbit hole: do not
try to filter stair materials.

---

## 6. Door activation: E-press + raycast, not proximity

### The fact
Doors are activated by an E-press plus a raycast, NOT by proximity.
`DOOR_REACH` is 200 units. When a door has no model bounds, a
fallback door box 160 wide/deep × 200 tall in local space is used
(`FALLBACK_DOOR_MIN` (-80,0,-80), `FALLBACK_DOOR_MAX` (80,200,80)).
`DOOR_BLOCKER_SLACK` is 15: a collider nearer than the door by more
than 15 blocks the view (the same test as their tankard grab).
A singular affine (XSCL 0/inf/NaN) makes the door not activatable.
`ray_hits_door` does a slab test in door-local space.

### Sources
- `crates/engine/src/door_crossing.rs` (2,070 lines).

### What our code does today
Our doors swing on proximity and the teleport fires on crossing
(the unit-tested `swinging_door_fires` predicate in `doors.rs`).
There is no activation key and no reach test.

### What should change
Nothing immediately — proximity + crossing is a legitimate mobile
adaptation (no keyboard on the phone), and our M15 doors work
outbound. But the raycast facts matter for the USE system: the
200-unit reach and the 15-unit blocker slack are the numbers to use
when we add tap-to-activate for doors and containers. The singular-
affine guard (XSCL 0/inf/NaN) is a cheap robustness check worth
copying wherever we resolve a door's transform.

---

## 7. Door crossing state machine — the checklist for our broken return

### The fact
The crossing is a state machine: FadeOut 0.25 s (`FADE_SECONDS`) →
at full black the space switches in ONE step (unload ALL old cells
at once, ActiveSpace + CameraSpace change, exterior landing sets
the render origin to the arrival grid, player teleported to the
XTEL arrival) → Hold black until the landing cell is resident +
nothing pending + every mesh/material/texture under the cell root
is loaded, then 2 more frames (`SETTLE_FRAMES`), max 10 s
(`LANDING_TIMEOUT_SECONDS`); the upload budget is lifted while
black. Failure/timeout → restore the player to the saved position
(the Restore is kept in ABSOLUTE feet coords so a render-origin
rebase can't move it); if restore fails → a Recovery stage, movement
paused until V or `tcl`. A WALK landing waits for safe capsule
placement + grounded, motionless controller output — not just render
residency. Movement systems pause for the whole crossing (a
`no_door_crossing` run condition); only input-free WALK physics
runs once landing assets have settled behind black. No preload.

The return trip is SYMMETRIC — no special-casing vs outbound.
`landing_for`: interior → absolute coords, origin unchanged,
CameraSpace::Interior; exterior → grid from the arrival position,
origin = arrival grid, CameraSpace::Exterior. `switch_space`:
`unload_all_cells_now` + set ActiveSpace/RenderOrigin/CameraSpace +
move camera immediately + `TeleportPlayer` message. `place_again`:
re-teleports at fade-in start (WALK keeps the settled pose; NOCLIP
re-placed on the landing point).

### Sources
- `crates/engine/src/door_crossing.rs` (2,070 lines).
- `crates/engine/src/doors.rs` (`DoorDestination` from XTEL).

### What our code does today
Our outbound trip works (M15); the RETURN trip (exterior→inn) is
CONFIRMED BROKEN on the phone (v0.4.6, 2026-11-15). We have a
loading screen + chunked spawn, but no documented equivalent of:
(1) unload-all-at-black, (2) hold-until-resident-and-settled,
(3) absolute-coords restore on failure, (4) movement pause across
the crossing.

### What should change (inn-return fix — FIRST, before collision)
Use this as the diagnosis checklist. The symmetry point is the key
insight: if outbound works and return doesn't, the bug is in
something the two directions do DIFFERENTLY in our code — the
prime suspects are the render-origin rebase on exterior landing
(their `landing_for` re-bases the origin on exterior arrival and
does NOT re-base on interior arrival) and the landing-residency
wait (our chunked spawn may report "done" before the interior cell
is actually resident). The absolute-coords restore is the safety
net to add regardless: a failed crossing must put the player back
where they were, not strand them.

---

## 8. XTEL decode + the door-links table

### The fact
`DoorDestination` from XTEL: bytes 0..4 destination ref, 4..16
arrival position, 16..28 arrival rotation. Exactly one of
`interior_cell_id` / `worldspace_id` must be Some for a usable
door; (0,0) = no door. Their converter builds a `door_links` table
from the merged winning records, so a mod that moves a door is
already applied.

### Sources
- `crates/engine/src/doors.rs`.
- `crates/converter/src/esm/doors.rs` (`project_door_links`).
- `crates/engine/src/world/database.rs:79` (`DoorLinkRow`).

### What our code does today
We decode XTEL the same way (32 bytes; destination, arrival pos,
arrival rot) and resolve doors both ways from the live ESM. Our
layout matches theirs exactly.

### What should change
Nothing on the decode. The "exactly one of interior/worldspace"
invariant is a good assertion to add to our door resolution: a door
with neither (or both) is corrupt data, and failing loudly beats
teleporting the player to (0,0,0).

---

## 9. `.hkx`: a solved packfile — retail SE uses only THREECOMP40

### The fact
`.hkx` is a Havok 2010.2 64-bit packfile: magic `0x57E0E057` /
`0x10C0C010`; version `hk_2010.2.0-r1`; FILE_VERSION 8; POINTER_SIZE
8; a 64-byte header + 48-byte section headers; exactly 3 sections
(`__classnames__`, `__types__`, `__data__`); pointer slots written
zero, filled from fixup tables.

Class layouts: hkRootLevelContainer namedVariants hkArray @0,
24-byte variants (name ptr @0, className ptr @8, variant ptr @16).
hkaAnimationContainer: skeletons @16, animations @32, bindings @48
(pointer arrays). hkaSkeleton: name @16, parentIndices @24, bones
@40; hkaBone 16 bytes (name ptr @0, lockTranslation @8).
hkaAnimationBinding: originalSkeletonName @16, animation ptr @24,
transformTrackToBoneIndices @32; blend_hint = i32 @64 (0 normal, 1
additive). hkaAnimation @16..56; hkaSplineCompressedAnimation
@56..176: duration f32 @20, numTransformTracks @24, numFloatTracks
@28, extractedMotion ptr @32, numFrames @56, numBlocks @60,
maxFramesPerBlock @64, frameDuration @80, blockOffsets @88,
floatBlockOffsets @104, data @152, endian @168.

Spline decode: per-block masks (4-byte mask per transform track, 1
mask byte per float track, align 4; then per track
translation/rotation/scale each align 4). Rotation quantisation: 0
Polar32, 1 ThreeComp40, 2 ThreeComp48, 3 ThreeComp24, 4 Straight16,
5 Uncompressed — **retail Skyrim SE uses ONLY THREECOMP40**. Scalar
quant: 0 Bits8, 1 Bits16. Mask bits: STATIC 0x01/02/04/08, SPLINE
0x10/20/40/80. Sampling: one sample per integer frame
0..frame_count; fps = 1/frame_duration (their test fixture: 30 fps).

### Sources
- `crates/converter/src/animation/` (`mod.rs`, `classes.rs`,
  `packfile.rs`, `spline.rs`, `glb.rs`, `authoring.rs`,
  `idle_defaults.rs`).

### What our code does today
Nothing: we have no `.hkx` reader. NPCs are static bind-pose
models. (Animations.bsa, 8,920 `.hkx` files, is in our test-data.)

### What should change (people-walking milestone)
This is the format map for a `dovah-hkx` crate (live parsing, like
`dovah-nif` — no conversion). The THREECOMP40-only fact bounds the
work: implement that one rotation path first, with the other
quantisations as later fallbacks. Sample one pose per integer frame
at 1/frame_duration fps; the engine side is §10.

---

## 10. Actor animation runtime: pose-only clips, speed drives gait

### The fact
Their converter writes each gait clip as a separate GLB over the
body's exact hierarchy — `mt_idle.glb`, `mt_walkforward.glb`,
`mt_runforward.glb` per sex at
`meshes/actors/character/animations/{male,female}/`. The engine
rebuilds node paths + `AnimationTargetId`, one `AnimationPlayer` on
the actor root with an `AnimationGraph` holding idle/walk/run.
Missing idle → bind pose; missing walk/run → nearest gait fallback.
Wrapper node name: `"Creation-to-glTF basis"`.

**NO root motion** — `NPC Root` is identity in every clip and
`extractedMotion` is null; the clip supplies pose only, displacement
is `speed × dt` along facing. Gait thresholds (their own
`MovementTuning`): walk_below 160, run_at_or_above 300 (Creation
u/s); `GAIT_TRANSITION` 200 ms crossfade; `gait_for`: ≤0/non-finite
→ Idle, ≥300 → Run, else Walk; an unchanged gait never restarts
the clip. `ActorDrive { speed }` is the only input; no AI/navigation
feeds it yet.

### Sources
- `crates/engine/src/actor_animation.rs` (1,356 lines).
- `crates/engine/src/actor_drive.rs`.
- `crates/converter/src/animation/glb.rs` (clip GLB layout; binding
  by bone name with case-aliasing).

### What our code does today
Static bind-pose NPCs. No animation player, no gait system, no AI.

### What should change (people-walking milestone)
The architecture transfers directly, minus the GLB step: parse
`.hkx` live (§9) into Bevy `AnimationClip`s over the body's bone
hierarchy, one `AnimationPlayer` per actor, gait from speed with
the 160/300 thresholds and a 200 ms crossfade. The no-root-motion
fact is the big simplification: we do NOT need to solve root-motion
extraction to get people walking — pose + `speed × dt` is what the
retail data supports. AI/schedules (where NPCs walk TO) is a
separate milestone after this one; `ActorDrive { speed }` is the
seam between them.

---

## 11. Water data: XCLW/XCWT + WRLD defaults + the FLT_MAX sentinel

### The fact
A CELL's `XCLW` is its water height, `XCWT` its water type. Most
exterior cells don't state a height: they carry `XCLW` = FLT_MAX (or
none), meaning "the worldspace's default". That default is the
second float of the worldspace's `WRLD/DNAM` (Tamriel: -14000, the
sea and the marshes around Morthal), with `WRLD/NAM2` as its water
type. Without it every such cell had no water: **10,630 of
Tamriel's 11,187 exterior cells**, including the open sea. Worldspace
markers 9,999,999 (MossMotherCavernWorld) / −500,000
(DeepwoodRedoubtWorld) mean no water. `WRLD/NAM3` is the LOD water
type. A cell whose terrain stays above the default draws no surface
anyway.

WATR `DNAM` is 228 bytes (30 records) or 232 bytes (8 SSE records,
an extra trailing Flowmap Scale float); none of the fields below
move between the two. Layout (xEdit's Water layout, confirmed
against Skyrim.esm/Update.esm): Reflectivity Amount f32 @20,
Fresnel Amount f32 @24, shallow colour u32 @40, deep colour u32 @44,
reflection colour u32 @48. Colours are little-endian RGB + pad byte.

### Sources
- `crates/converter/src/esm/cell_cache.rs` (`water_by_cell`, ~:351+;
  `normalize_water_height` / `normalize_default_water_height`).
- `crates/converter/src/esm/exporter.rs:935` (WATR export),
  `:1285-1330` (`parse_water_dnam`).
- `crates/converter/src/esm/environment.rs:492,538-544` (XCWT/NAM2/
  NAM3 links, WRLD DNAM floats).
- `crates/engine/src/world/database.rs:237-244` (`WaterColors`).

### What our code does today
Nothing: no `XCLW`/`XCWT`/`WATR` reading anywhere in our crates
(grep-confirmed). Our river is dry (Dylan, 2026-11-15).

### What should change (water milestone)
Read XCLW/XCWT per cell + WRLD DNAM/NAM2 defaults, with the
FLT_MAX sentinel and the two no-water markers. Resolve WATR DNAM
for colours/fresnel/reflectivity. DefaultWater's deep colour is
srgba_u8(5,14,18,153) — 60% cover, so beds show without depth fog.
Caveat: their `docs/research/water.md` (cited by exporter.rs) is
NOT in this clone — the DNAM layout above rests on the code + its
tests, not that doc.

---

## 12. Water rendering: one flat plane per cell + a real water shader

### The fact
One flat `Plane3d` per cell, size CELL_SIZE × CELL_SIZE, at the
cell's water height; only spawned when terrain dips below it
(`FlatWater::from_terrain`). Material:
`ExtendedMaterial<StandardMaterial, WaterExtension>`, metallic 0.15,
perceptual roughness 0.06, reflectance 0.9, `AlphaMode::Blend`, on
`WATER_LAYER`. Defaults: fresnel 0.10 / reflectivity 0.8.

The shader (`water.wgsl`): procedural wave normal perturbation
(cos/sin, wave scale/speed/strength) + optional scrolling flow
normal map (`fract(p * 0.25)`); planar reflection texture blended
with Schlick fresnel (F0 = WATR "Fresnel Amount" DNAM); reflection
fades to the water's Reflection Color where the reflection image
has no coverage (alpha 0); **reflection blended AFTER lighting**
(lighting it as albedo washed the water white).

### Sources
- `crates/engine/src/streaming.rs:1735+` (surface spawn).
- `crates/engine/src/world/water.rs:13+` (`FlatWater`).
- `crates/engine/src/shaders/water.wgsl`.
- `crates/engine/src/render.rs:703-732` (`WaterExtension`).

### What our code does today
No water surface at all. (Our transparent pass exists since the
exterior follow-up, so the Blend material has somewhere to go.)

### What should change (water milestone)
Adopt the shape: flat per-cell plane at the resolved height,
spawned only where terrain dips below it; WATR-driven colours +
fresnel; wave normals + scrolling flow map in a custom shader;
reflection blended after lighting. The "blend reflection after
lighting" note is the kind of trap that costs a day if learned by
trial: record it in the milestone brief.

---

## 13. Water reflection: a second camera over the LOD world

### The fact
Vanilla Skyrim builds its water reflection from the LOD world —
LOD land, LOD objects, LOD trees (SE adds screen-space reflections
for near geometry, which Mudcrab does not have). Mudcrab mirrors
this: a reflection camera renders the LOD world into a reflection
texture (`WaterReflectionTexture`), and the water shader samples
it. The reflection camera chooses ONE mirror plane for its whole
view. Reflection-view layer membership is explicit
(`REFLECTION_VIEW_LAYERS`); placed objects are in the reflection
because they sit on `PLACED_OBJECT_LAYER`.

### Sources
- `crates/engine/src/render.rs:55-147` (reflection camera setup),
  `:142-147` (systems).

### What our code does today
No reflection pass, no LOD world to reflect.

### What should change (water milestone, second half)
Planar reflection is the expensive half of the water milestone. The
pragmatic order: flat plane + shader first (no reflection — the
fresnel blend fades to Reflection Color where coverage is absent,
which is exactly the no-reflection fallback), then the reflection
camera once LOD exists (§18) — reflecting the full-detail world
would double the draw cost for no vanilla reason. Do NOT attempt
screen-space reflections.

---

## 14. Water queries: FlatWaterBody + water_at_point (for swimming, later)

### The fact
`FlatWaterBody` lives on the streamed `WaterSurface` entity (no
separate registry). `water_at_point` returns the covering CELL
identity, flat surface height, and optional WATR FormID for a
`WorldPosition`. It does NOT test submersion — a point above the
surface still returns the covering body; the caller compares Z
with `surface.height`. Within a cell, coverage requires positive
depth above the detailed LAND mesh (interpolation uses the
renderer's `[a,b,c]`, `[b,d,c]` triangles, including their
diagonal); the shoreline itself is dry. Overlapping coverage
chooses the highest surface; equal heights choose the highest CELL
FormID. (That overlap rule is their explicit policy, NOT
established as Skyrim's native rule.) The cache does not carry
authored water polygons, interior pool bounds, or coverage for
water without LAND — those must not be inferred.

### Sources
- `crates/engine/src/world/water.rs` (`FlatWaterBody`).
- `docs/specs/engine/flat-water.md`.

### What our code does today
No water queries (no water).

### What should change
Nothing until swimming exists. When it does, this is the query
shape: coverage on the surface entity, positive-depth test against
the rendered LAND triangles, caller-side submersion check. The
"shoreline is dry" rule is the detail that makes wading feel
right.

---

## 15. Mist/foam: NO dedicated system — the effect-palette shader IS the mist

### The fact
There is no mist or foam code in Mudcrab — the grep for mist/foam/
newFog returns only false positives (mistyped-option strings).
What handles the waterfall mist is the generic
`BSEffectShaderProperty` path: Greyscale_To_PaletteColor /
Greyscale_To_PaletteAlpha. The source texture's grey level picks a
colour (and alpha) from a palette texture instead of being drawn:

- colour = base colour scale × palette(texture.g, base colour.r ×
  vertex.r).rgb
- alpha = palette(texture.a, base colour.a × vertex.a).a

Effect shaders are unlit — the result is drawn as-is. The palette
lookup runs in `effect_palette.wgsl` (with `effect_color` /
`source_texel` helpers); the material is an unlit extended
material. Fire-refraction surfaces are explicitly EXCLUDED, not
rendered as opaque cards.

### Sources
- `crates/engine/src/shaders/effect_palette.wgsl` (the lookup
  formula is in the file header).
- `docs/lighting-l1-compatibility.md` (Effect shader row: palette
  colour/alpha drawn unlit; particle/falloff/refraction/animation
  semantics still missing).

### What our code does today
Our `newFog` sheets render as opaque white (mod-tools-reference.md
§9 identified them; the exterior follow-up's transparent pass made
them a faint veil, but the mist STACK still reads as white sheets
on the phone). Our effect-shader handling multiplies a gradient
into the texture — the wrong mechanism.

### What should change (water milestone, with §12)
Implement the palette lookup: read the palette texture from the
effect shader property, sample it by (texture grey, base-colour-r
× vertex-r) for colour and (texture alpha, base-colour-a ×
vertex-a) for alpha, draw unlit. This one shader change is the fix
for the white sheets — no mist-specific code is needed, and
Mudcrab's empty grep proves the point. The palette texture path
comes from the NIF's effect property; verify it resolves from our
BSAs before the milestone starts.

---

## 16. Streaming: radius, budgets, and the Retiring state

### The fact
Stream radius default 2 (= Skyrim's `uGridsToLoad=5`, a 5×5 grid);
unload radius = stream radius + 1 (default 3). Per-frame budgets:
max 1 cell commit, max 16,670 µs commit time, max 2 cell unloads,
max 32 model spawns (the 32 came from measurement: a post-jump
backlog of ~2,000 models took ~500 frames at 4/frame; 32 clears it
2.5× sooner with the same worst frame). Cell states:
Loading / Resident / Retiring / Failed. A Retiring cell (outside
the unload radius, waiting for its unload turn) STAYS DRAWN and its
map entry stays, so a cell that comes back into range before its
turn is revived as-is instead of being despawned and re-requested.
The retire-backlog bound is one whole window at the unload radius,
`(2r+1)²` cells; past it, every retiring cell unloads at once.
`unload_all_cells_now` exists for worldspace switches (§7). There
is also an adaptive-streaming mode (opt-in): spatial priority
queueing, backlog watermarks, memory admission.

### Sources
- `crates/engine/src/config.rs:84-94,232-238` (radii + budgets),
  `:2999` (layered (5,6) test).
- `crates/engine/src/streaming.rs:812-830` (`CellStatus`),
  `:1051-1140` (retire pacing).

### What our code does today
We load a FIXED 3×3 grid (x∈[4,6], y∈[-12,-10]) with a chunked
scene build (8 ms/frame budget, Spawning phase). No radius, no
unload, no streaming at all — the player cannot leave Riverwood.

### What should change (world-streaming milestone, LATER)
The numbers transfer: stream radius 2 / unload 3, one commit per
frame, 2 unloads per frame. The Retiring state is the design idea
worth copying — it makes radius-crossing hysteresis free. Our
chunked-build machinery (SpawnProgress, 8 ms budget) is the
natural home for the commit budget; the streaming milestone adds
the planner + the R-tree index (§17), not a new build path. This
is later work — do not start it before Riverwood-complete.

---

## 17. LOD: fixed 4/8/16-cell tiers, GLB chunks, R-tree index

### The fact
LOD is selected by FIXED spatial tiers past the full-detail grid:
Tier4 = 4×4-cell chunks (nearest ring), Tier8 = 8×8, Tier16 = 16×16
(farthest compiled tier). Distances come from SkyrimPrefs.ini
defaults: `fBlockLevel0Distance` 35,000, `fBlockLevel1Distance`
70,000, `fBlockMaximumDistance` 250,000, `fSplitDistanceMult` 1.5 —
about 12, 25, and 91 cells for tiers 4, 8, 16. Chunks ship as GLB
files through the asset pipeline, with the database holding an
INDEX, not blobs: chunk key (world, tier, anchor), bounds, batch
list, content hashes, manifest references — additionally indexed
in an R-tree over world bounds so range queries stay spatial.
LOD streaming has its own machinery: unload margin 2 cells, chunk
retries (max 3, backoff 1s→4s), a far plane that keeps every cell
within reach (`CELL_SIZE × (reach + 0.5) × √2`). A hybrid
projected-error object LOD (meshoptimizer cluster selection by
projected geometric error, ~1px experimental setting) is PROPOSED
only — not shipped. Terrain LOD quadrants are a triangle fan:
65 verts (perimeter + one interior centre), 192 indices.

### Sources
- `crates/shared/src/lod.rs:14-21` (`LodTier`), `:23-28` (quadrant
  fan counts).
- `crates/engine/src/config.rs:26-46` (`TerrainLodDistances`).
- `docs/adr/0011-lod-tier-selection-policy.md` (fixed tiers;
  hybrid extension proposed).
- `docs/adr/0010-lod-chunk-payload-format.md` (GLB chunks + R-tree).
- `crates/engine/src/streaming/lod.rs` (unload margin, retries,
  `lod_camera_far`).
- `docs/research/lod-consolidation-20260930.md`,
  `lod-conversion-implementation-20261009.md` (conversion detail —
  not read in depth; the conversion side does not apply to us).

### What our code does today
No LOD at all: full-detail meshes to the edge of our 3×3, then
nothing. (NiSwitchNode child selection, M13, suppresses the in-NIF
LOD child — that is per-mesh LOD, a different layer.)

### What should change (world-streaming milestone, LATER)
The tier policy transfers (4/8/16, INI distances); the GLB-chunk
storage does NOT — our LOD chunks would be built live from LAND +
placed refs at load, or baked once into our own cache format.
The R-tree idea transfers as "index chunks spatially, don't
enumerate anchors". The quadrant fan (65 verts) is the cheapest
correct terrain-LOD mesh shape. The hybrid cluster LOD is
experimental even for them — ignore it.

---

## 18. Merged terrain: one mesh per cell, one material for all

### The fact
Their merged terrain path draws one mesh per exterior cell with
ONE shared material for all cells: per-cell `TerrainCellMesh`
(four unwelded quadrants with per-vertex texture slots), game-wide
land texture arrays + a lookup table. Any cell without usable
merged data — and every cell when the merged path is unavailable —
falls back to the per-quadrant path; nothing is dropped silently,
and the streaming metrics count both kinds.

### Sources
- `crates/engine/src/terrain_merge/mod.rs` (module doc),
  `mesh.rs`, `material.rs`, `terrain_merged.wgsl`.

### What our code does today
One mesh per cell already (`land.rs:147` `terrain_mesh`), but one
BAKED texture per cell (1024², `vtxt.rs`) + one material per cell.
Nine cells = nine materials, nine 4 MB textures.

### What should change (later optimisation, NOT Riverwood-complete)
Their texture-array + one-material design is the right end state
for a streamed world (hundreds of cells can't each carry a baked
1024² texture + material). For Riverwood's 3×3, our bake is fine.
File this under the streaming milestone: when cells stream, switch
from per-cell baked textures to texture arrays + per-vertex layer
slots. Do not rework the bake now — §19's tiling fix is the only
terrain-texture change the current milestone needs.

---

## 19. Terrain texture scale: 24 repeats per cell — OUR BAKE USES 8

### The fact
The default LAND texture frequency is **24 repeats per full cell
per axis**, 12 across a quadrant. Evidence: Sheson's xLODGen
explanation (24×24 tiles per cell, 96×96 per 4-cell LOD texture);
`fLandTextureTilingMult=3.0` (SkyrimPrefs default); and a read-only
static inspection of SkyrimSE.exe 1.7.104.0 recovering the
calculation: `step = 1 / (4 / setting) = 3/4`, `quadrant_uv(x, y)
= (x × step, y × step)`, x,y in 0..16 — 16 intervals give 12
repeats across a quadrant, 32 give 24 across a cell. This scales
diffuse/normal sampling ONLY; it does not rescale the 17×17
quadrant paint-weight grid.

### Sources
- `docs/specs/engine/landscape-texture-scale.md` (full derivation).
- `crates/shared/src/lib.rs` `LAND_TEXTURE_REPEATS_PER_CELL`
  (`32.0 × (3.0/4.0)` = 24.0).

### What our code does today
`crates/dovah-app/src/vtxt.rs:39` `TILE_QUADS = 4.0`: the bake
computes tile coords as `vx / 4.0` with vx spanning 0..32 across
the cell — **8 repeats per cell**. Our ground textures are 3×
larger/coarser than the game's.

### What should change (Riverwood-complete, terrain pass)
Set `TILE_QUADS = 32.0 / 24.0` (≈1.333) and re-render the exterior
against the current proof shot. At a 1024² bake, 24 repeats means
~43 px per tile — no resolution problem. Verify by eye: the dirt
path and grass should read finer and match the game's density.
This is a one-constant change with a render check, not a research
project — but do the render check, because our bake also blends
layers by opacity and the tiling interacts with the blend.

---

## 20. Interior room graph: RoomMarker/PortalMarker + per-room lighting

### The fact
Skyrim's interiors carry room markers and portal markers as
placed references. Bases: `RoomMarker` 0x1F (3,561 placed refs in
Skyrim.esm), `PortalMarker` 0x20 (2,612). Subrecords on a marker
ref, besides NAME/DATA:

| Subrecord | Size | Layout | On |
|---|---|---|---|
| `XPRM` | 32 | `float[3]` half-extents, `float[3]` colour/255, `float` unknown (0.2 room, 0.25 portal), `u32` shape (1 box, 2 sphere, 3 portal box) | every room, 2,611 portals |
| `XMBO` | 12 | `float[3]`, equal to `XPRM[0..12]` on every room | every room |
| `XPOD` | 8/pair | array of (origin, destination) `u32` FormID pairs: the rooms on either side; masters only have one pair | 2,592 portals (121 with second id 0, 20 without `XPOD`) |
| `XRMR` | 4 | `u8` count of `XLRM` that follow, `u8` flags, `u16` unknown (0 or 1) | 1,991 rooms |
| `LNAM` | 4 | `u32` FormID: lighting template for the room | follows `XRMR` when flag `0x80` set |
| `INAM` | 4 | `u32` FormID: image space for the room | follows `XRMR` when flag `0x40` set |
| `XLRM` | 4 | `u32` FormID of another room in the same cell | 1,965 links |

Checked against bytes: the `XRMR` count equals the `XLRM` count on
every room; flags match the public names exactly (0x80 with LNAM
on 618 rooms, 0x40 with INAM on 86) — **rooms can carry their own
lighting template and image space**. `XLRM` links: all 1,965 point
to a room in the same cell, 1,950 mirrored, and only 25 join rooms
that already share a portal — so a link joins rooms with NO portal
between them that must be seen together ("linked rooms"): treat a
link as an open edge (if one room is visible, so is the other, with
no narrowing). Room membership is NOT stored on objects: an object
belongs to the room(s) whose box contains its pivot (measured:
90.6–99.8% of refs fall in ≥1 room, 1.2–13.2% in more than one).
A portal's `XPRM.y` is always 1.0 (thickness placeholder); its
doorway is the marker's local X-Z rectangle (normal along local
+Y). Coverage: 246 of 590 interiors have room markers, 94 have 16
or more. 159 rooms / 133 portals sit in exteriors (out of scope).

### Sources
- `docs/research/interior-room-graph-converter.md` (all layouts
  checked against Skyrim.esm bytes + UESP REFR + xEdit #919).

### What our code does today
Nothing: we don't read RoomMarker/PortalMarker, and our interior
rendering has no culling and no per-room lighting (one INDOOR
exposure + one ambient for the whole inn).

### What should change (all-interiors milestone)
Two separate takes: (1) PORTAL CULLING — decode the room graph
live, assign refs to rooms by pivot-in-box, cull by portal
visibility (objects in no room or several rooms stay drawn — the
safe default). (2) PER-ROOM LIGHTING — LNAM lighting templates and
INAM image spaces are real per-room data; the inn's rooms may
already differ in the data. The lighting half is the higher-value
take for Riverwood-complete (interiors that feel different room
to room); culling is a performance optimisation for later, when
interiors get big. Record the subrecord layouts in
`docs/formats/` when the milestone starts.

---

## 21. XESP enable-parent rule: the parent's state wins

### The fact
The Creation Kit rule: a child's disable/enable state is ALWAYS
determined by its enable parent (or the inverse when "Set Enable
State to Opposite of Parent" is checked). The child's own
`Initially Disabled` header bit (0x800) does NOT override an
existing enable parent. XESP is a parent FormID + a one-byte flags
field + three unused bytes; flags bit 0 = Opposite of Parent,
bit 1 = Pop In. Spawn-state evaluation inspects bit 0 only. FormID
0x14 is the hardcoded PlayerRef (absent from plugin records);
treating it as enabled in a clean new game is an implementation
ASSUMPTION, not a source-verified fact. Deleted records are
recognised by header bit 0x20 and removed during plugin merge.
Their converter reports 42,395 resolved XESP parents across 80
official plugins, 16 parents pointing at 0x14. Unresolved: a
nonzero parent missing from the database, a deleted parent, or an
XESP cycle — no documented behaviour exists for those.

### Sources
- `docs/research/reference-enable-state-20261005.md` (CK wiki
  revision pinned + hashed; TES5Edit XESP definition).
- `docs/research/reference-enable-retail-check-20261009.md`.

### What our code does today
Our `ref_enabled` resolves XESP parents (the exterior follow-up:
the 9th actor was a saddled horse whose parent E3demoMarker is
disabled). Whether we apply the bit-0 inversion and the
parent-wins-over-0x800 rule was not re-checked in this study.

### What should change (small verification item)
Re-read our `ref_enabled` against this rule: parent state wins
over the child's own 0x800 bit; bit 0 inverts; bit 1 (Pop In) does
not change the initial state; 0x14 counts as enabled. If any of
those is missing, it's a one-line fix each. The unresolved cases
(missing/deleted/cyclic parent) should resolve to "omit + log",
their conservative policy.

---

## 22. XRDS: per-reference light radius overrides — WE DON'T READ THEM

### The fact
A `LIGH` reference's `XRDS` subrecord is a radius override that
WINS over the record's radius. 10,810 of the 12,148 `LIGH`
references in Skyrim.esm carry one. An override that is not a
positive finite number is ignored (XRDS has been seen negative).
An override above `MAX_RADIUS_OVERRIDE` (8,192 units, two exterior
cells) is ignored too: the base game's positive overrides run up
to 6,919 and then jump to nine outliers (15,967 to 3,736,737) that
would reach across many cells. The record's radius is used when
the reference has no usable override.

### Sources
- `docs/specs/engine/point-lights.md` §1.
- `crates/engine/src/world/database.rs` (`radius_override` column).

### What our code does today
We do NOT read XRDS (grep-confirmed: zero hits in `crates/` or
`docs/formats/`). Our M14/M16 lights use the record radius only
(all 8 inn bases: 512u).

### What should change (lighting follow-up)
Read XRDS per reference, with their guards (positive + finite,
≤ 8,192, else fall back to the record radius). The inn's lights
may or may not carry overrides — check the 8 inn LIGH refs
specifically; if any do, our interior lighting is currently wrong
for those lights. Record XRDS in `docs/formats/`.

---

## 23. LIGH flags that place no light + their intensity model

### The fact
Two `LIGH` DATA flag bits place no light: 0x00000004 negative (the
light REMOVES light; a clustered renderer can't subtract it),
0x00000020 off by default (dark until a script turns it on).
Neither bit is set by any `LIGH` record in the shipped plugins —
they exist for mods and scripts.

Their intensity model: Bevy's point light contributes
`albedo × NdotL × colour × intensity × window(d) / (4π²d²)` with
`window(d) = (1 − (d/range)⁴)²` (the 4π² is Bevy's: 4π CPU division
+ Lambert 1/π). They solve the intensity that delivers 10× the
ambient illuminance at half the light's radius:
`intensity = 4π² × 160 × 10 × (radius/2)² / (225/256)`.
`LIGHT_EXPOSURE = 10` is "a round default, not a fitted number".
A 512-unit torch gets ≈ 4.7e9. A reference cap
(`INTENSITY_REFERENCE_RADIUS = 256`) stops big radii from becoming
bright: a light with radius > 512 is lit at 256 whatever its
radius — it keeps its whole RANGE, but its brightness stops
growing with the square of its reach. (A radius-4334 record came
out 71× a torch before the cap.) Their lights are opt-in behind
`--lights`, and `shadow_maps_enabled` is FALSE — one cube map per
light is unaffordable at 64 lights; honouring the shadow flags is
left for later.

### Sources
- `docs/specs/engine/point-lights.md` §§3–5.
- `crates/engine/src/lights.rs`.
- `crates/converter/src/esm/exporter.rs` (LIGH DATA layout comment:
  48 bytes, all 435 LIGH records in Skyrim.esm).

### What our code does today
Our model is DIFFERENT and render-fitted: linear falloff via the
vendored `bevy_pbr` shader (`getDistanceAttenuation` →
`saturate(1 − d/R)`), intensity = 30,000 × Fade (LIGH FNAM),
calibrated from the render. We DO honour shadow flags (0x1000
omni → shadow-casting point light, cap 4/scene, 512px faces) —
we are AHEAD of them there. We do not skip 0x4/0x20 lights (moot
for the shipped game: no record sets them).

### What should change
Nothing on intensity — ours is render-calibrated, theirs is an
unfitted analytic default; do not replace a fitted number with an
unfitted one. Two takes: (1) the radius-independence insight —
our 30,000 × Fade is already radius-independent, which is the same
conclusion reached from the other direction; record that as
validation. (2) Their `LIGHT_DATA_MIN = 20` acceptance (read the
first 20 bytes, ignore the tail) is a robustness pattern worth
copying if we ever accept short LIGH DATA.

---

## 24. Sprint: an honest partial result — the 1.5× is provisional

### The fact
Their sprint investigation (2026-10-15) is explicitly a PARTIAL
result, not a recovered equation. Established: RACE carries six
nullable MOVT links — WKMV, RNMV, SWMV, FLMV, SNMV, SPMV (xEdit
definitions); CommonLib's `MaxSpeeds` has left/right/forward/back/
rotation pairs indexed walk/run, plus rotate-while-moving-run —
there is NO separate sprint float in the structure. The native
report (#246, SkyrimSE.exe 1.7.104.0) says DMSP → MOVT 0x000F3469
(`AIControlledNPC_Sprinting_MT`); SPMV precedes DMSP at the race
base-slot getter; DOBJ merges by key. NOT established: which
consumer uses which slot, whether sprint uses the run column
directly, the speed equation, stamina/encumbrance participation.
Their runtime multiplies run × 1.5 (`sprint_multiplier` default)
and labels it "an explicitly provisional legacy fallback, not a
successful resolution". Their directional blend is also
provisional: `base = (lateral × |sx| + longitudinal × |sy|) /
max(|sx| + |sy|, 1)`.

### Sources
- `docs/player-sprint-research.md` (full evidence table + the
  required native-trace experiment list).

### What our code does today
No sprint, no MOVT reading, no stamina.

### What should change (movement milestone, after collision)
Take the 1.5× as the provisional sprint multiplier and the six
MOVT slots as the data to read — but do NOT present either as
retail-proven. Their experiment list (procedural MOVT overrides
with distinct values, one input at a time, measuring steady-state
displacement) is the right way to settle it later, and it needs a
retail install + measurement harness we don't have. The honest
framing matters: Dylan wants exact Skyrim, and an unproven
multiplier presented as fact is worse than a labelled placeholder.

---

## 25. L1 material compatibility: their taxonomy mirrors our gaps

### The fact
Their L1 (material correctness) table separates what is projected
from what is missing. Implemented + tested: diffuse transfer
space, linear factors, specular enable, normal-alpha mask,
exponent-to-roughness approximation, authored vertex channels, UV
scale/offset + S/T wrapping; tangent-space normals (DirectX Y
applied ONCE in native material construction); glow/static
emission (eligible slot, tint, multiplier); alpha test / ordinary
blend (NiAlphaProperty determines mode; visible/depth/shadow
discard agree); effect-palette shader (§15). Still missing:
native BRDF + intensity, authored tangent frames, model-space
normal shader path (1,123 materials — currently approximated with
the tangent interpretation, which "blocks parity"), animated
emission controllers, simultaneous blend/test, nonstandard blend
factors, full transparency ordering, tree animation, authored
LOD/fade, env-map/eye-map response, parallax (11 + 682 multilayer
materials), face/skin/hair tint + SSS, snow coverage + sparkle,
specialised NIF landscape/cloud paths. Two exclusion rules worth
noting: shapes with the `NiAVObject` Hidden flag (or under a
hidden node) are excluded with a recorded reason (weapon blood
overlays, emitter + baked particle geometry); EditorMarker shapes
with the BSX editor flag are excluded. Their converted corpus:
25,388 GLBs, 63,981 materials; alpha modes 45,075 opaque / 13,035
mask / 5,871 blend; 3,488 materials carry nonidentity UV
transforms.

### Sources
- `docs/lighting-l1-compatibility.md` (full table + inventory).
- `docs/specs/engine/color-pipeline.md` (effect-palette §289).
- `docs/adr/0009-cutout-vertex-alpha-normalization.md` (superseded
  workaround — the same vertex-alpha problem our M13 fixed by
  forcing cutout vertex alpha to 1.0).

### What our code does today
Our material path covers the same implemented set (diffuse,
normals with BOTH channels negated — render-proven, vs their
DirectX-Y-once; Mask/A2C/Blend by NiAlphaProperty; vertex colours).
Our known gaps match theirs: skin SSS (faces read dark), model-
space normals, parallax, snow. We do NOT exclude Hidden-flag
shapes or EditorMarkers (not verified in this study — check).

### What should change
(1) Verify whether we exclude `NiAVObject`-Hidden shapes and
EditorMarker+BSX shapes; if not, add the exclusion (hidden emitter
geometry rendering as visible junk is a plausible source of the
yellow/green placeholder boxes Dylan photographed). (2) The
normal-convention note: 1,123 model-space materials exist — our
"both channels negated" rule was proven on tangent-space normals;
if a model-space material renders wrong, the convention flag is
the first suspect. (3) Their ADR 0009 confirms our M13
vertex-alpha fix independently — same problem, same conclusion.

---

## 26. Tone mapping / colour pipeline: record grading + frozen references

### The fact
Their colour grading reads IMGS (image space) records: CNAM/TNAM
selection picks the grading; variants are `record-srgb` (authored
grading from a loaded record — their example loads form 0x12F87,
winner plugin Update.esm) vs `snapshot-srgb` (the approved
default). Visual profiles carry sky/fog presets (e.g. "cool-gray").
Their tone-map tuning process is the notable METHOD: every
rendering change keeps three images available (previously approved
Mudcrab setup, fixed Skyrim screenshot, current capture); a runner
script preserves references and captures named variants from ONE
camera + ONE INI, updating an HTML comparison after each validated
stage; the runner refuses source/executable changes during a
campaign; a successful capture validates the Mudcrab scene and its
receipts but "does not establish Skyrim fidelity" (the reference
screenshot's weather, exposure, adapted luminance, and GPU
constants were not observed).

### Sources
- `docs/specs/engine/tonemap-tuning.md`.
- `docs/specs/engine/color-pipeline.md`.
- `docs/specs/engine/visual-profiles.md`.

### What our code does today
Exposure presets (OVERCAST outdoors / INDOOR), ambient tuning,
render-vs-render md5 checks. No IMGS reading, no colour grading,
no frozen-reference process (our renders are compared ad hoc).

### What should change (later visual-fidelity pass)
Two takes: (1) IMGS record grading is a real system to build
eventually — the image-space record is how the game grades each
area, and it's the missing piece between our exposure presets and
the game's look. (2) Their frozen-reference method is worth
adopting NOW for the exterior: pin one camera + one INI + one
approved render, and judge every lighting/material change against
all three images. It costs a script, not a milestone, and it stops
"looks better to me" drift.

---

## 27. Corroborations: BSFadeNode, NiSkinPartition, NiAlphaProperty

### The fact
Three of our hard-won M11/M12 findings are independently
confirmed by their converter: `BSFadeNode` is treated as a node
(their mesh converter matches `NiNode | BSFadeNode` at the root);
`NiSkinPartition` parses under version 100 (SE) and FAILS under
version 83 (LE) — the SE layout difference is real and versioned;
NiAlphaProperty determines the alpha mode (their L1 table). Their
converter also carries the same "20 NiSkinInstance blocks" style
diagnostics we use.

### Sources
- `crates/converter/src/mesh.rs:4606` (BSFadeNode as node),
  `:3662-3673` (NiSkinPartition v100 ok / v83 err), `:1024,1350`
  (NiSkinInstance handling).

### What our code does today
All three already implemented (M11 BSFadeNode-as-node, M12 SE
skinning layouts, M11 alpha rules).

### What should change
Nothing. This section exists so nobody re-litigates these three:
two independent codebases, same conclusions.

---

## Ordered checklist — what the next milestones take from this study

**Inn-return fix (FIRST — before anything else):**
1. Diagnose the broken exterior→inn return against the §7 state
   machine: check the render-origin rebase on exterior arrival vs
   interior arrival, and whether our chunked spawn reports "done"
   before the landing cell is resident. Add the absolute-coords
   restore on failure.
2. Add the §8 invariant: exactly one of interior_cell_id /
   worldspace_id, else fail loudly.

**Collision milestone (Riverwood-complete, first system):**
3. Parse `bhkCollisionObject`/`bhkRigidBody` + the §1 shape
   families in `dovah-nif` (compressed-mesh first — the stairs
   need it). Skip layers 12/15; treat 31 STAIRHELPER as physical.
4. Terrain trimesh from the exact rendered mesh buffers (§4).
5. Kinematic capsule controller + custom step logic (§2) with the
   §3 numbers (28r/100.8h, gravity 900, jump 340, autostep 36,
   slopes 50°/55°). Reconcile eye height (89.6u vs our 1.7 m)
   against a render before changing.
6. Acceptance: bridge, walkway stairs, mill ramp walkable; porch
   roofs block (§5). Copy their stair envelope as tests (24u
   climbs, 48u blocked, ceiling check).
7. Adopt "authored absence never falls back" (§4) and the 256
   live-dynamic cap for later clutter physics.

**Water milestone (Riverwood-complete):**
8. Read XCLW/XCWT + WRLD DNAM/NAM2 defaults with the FLT_MAX
   sentinel (§11); resolve WATR DNAM colours/fresnel (§11).
9. Flat per-cell plane where terrain dips below the height +
   wave/flow shader, reflection blended AFTER lighting (§12).
   Reflection camera over the LOD world comes later (§13) — ship
   the plane first, fading to Reflection Color.
10. Implement the effect-palette shader (§15) — this is the mist
    fix. No mist-specific code.
11. `FlatWaterBody` query shape (§14) when swimming is scoped.

**Terrain pass (Riverwood-complete, small):**
12. `TILE_QUADS`: 4.0 → 32/24 (≈1.333) — 24 repeats per cell (§19).
    Re-render and compare against the current proof shot.

**All-interiors milestone:**
13. Decode the room graph (RoomMarker/PortalMarker, §20) — per-room
    lighting templates (LNAM) and image spaces (INAM) first,
    portal culling second.
14. Read XRDS radius overrides with the §22 guards; check the 8
    inn LIGH refs specifically.
15. Verify `ref_enabled` against the §21 parent-wins rule
    (bit-0 inversion, 0x14 enabled, Pop In ignored).

**People-walking milestone:**
16. `dovah-hkx` crate from the §9 map (THREECOMP40 first, 30 fps
    sampling).
17. Pose-only clips + `AnimationPlayer` + gait from speed
    (160/300 thresholds, 200 ms crossfade) — NO root motion (§10).
    AI/schedules are a separate milestone after this.

**Small verifications (any time, cheap):**
18. Check whether we exclude `NiAVObject`-Hidden shapes and
    EditorMarker+BSX shapes (§25) — a plausible source of the
    yellow/green placeholder boxes on the phone.
19. Check the model-space normal convention flag on materials that
    render wrong (§25).
20. Adopt the frozen-reference method (§26): one pinned camera +
    INI + approved render for the exterior, judged on every
    lighting/material change.

**Explicitly NOT taken:**
- The convert-first pipeline (SQLite/glTF/KTX2/Luau) — our
  architecture reads originals live; nothing in this study argues
  for changing that.
- Their analytic light-intensity model (§23) — ours is
  render-calibrated and already radius-independent; their 10× is
  unfitted.
- Their sprint 1.5× as fact (§24) — provisional in their own
  words; use it labelled as such.
- Hybrid cluster LOD (§17) — experimental even for them.
- Screen-space reflections (§13) — they don't have them either.

## Appendix: key numbers at a glance

| Quantity | Value |
|---|---|
| Havok → Creation units | ×70.0 (Havok values are metres) |
| Capsule radius / standing height / eye | 28 / 100.8 / 89.6 units |
| Walk / run / sprint | 160 / 300 / ×1.5 u/s |
| Gravity / jump launch / accel | 900 / 340 / 1800 u/s² |
| Autostep / ground snap / controller offset | 36 / 12 / 2.0 units |
| Slope climb / slide | 50° / 55° |
| MOVT Forward walk/run, Back walk/run | 80.10/370.00, 71.93/205.25 |
| NPC_Default_MT / AI sprint MT | 0x0003580D / 0x000F3469 |
| Door reach / fallback box / blocker slack | 200u / 160×200u / 15u |
| Door fade / settle frames / landing timeout | 0.25 s / 2 frames / 10 s |
| `.hkx` magic / version / sections | 0x57E0E057 / hk_2010.2.0-r1 / 3 |
| Retail SE rotation quantisation | THREECOMP40 only |
| Gait thresholds walk/run | 160 / 300 u/s; crossfade 200 ms |
| Tamriel default water height (WRLD DNAM) | -14000 |
| Cells using the water default | 10,630 / 11,187 |
| WATR DNAM size / fresnel @ / deep @ | 228 or 232 B / @24 / @44 |
| DefaultWater deep colour | srgba_u8(5,14,18,153) |
| Water fresnel / reflectivity defaults | 0.10 / 0.8 |
| Stream radius / unload radius | 2 / 3 cells (= uGridsToLoad 5) |
| Commit / unload / model-spawn budgets | 1 cell, 16,670 µs / 2 / 32 per frame |
| LOD tiers / INI distances | 4/8/16 cells; 35k/70k/250k × 1.5 (≈12/25/91 cells) |
| Land texture repeats per cell | 24 (12 per quadrant); ours: 8 |
| RoomMarker / PortalMarker form IDs | 0x1F / 0x20 (3,561 / 2,612 refs) |
| Interiors with room markers | 246 / 590 |
| LIGH refs with XRDS override | 10,810 / 12,148 |
| XRDS cap / outlier range | 8,192u / 15,967–3,736,737 |
| XESP parents resolved (80 plugins) | 42,395; 16 point at PlayerRef 0x14 |
| Alpha-mode material counts (their corpus) | 45,075 opaque / 13,035 mask / 5,871 blend |
| Max live dynamic clutter | 256 |
