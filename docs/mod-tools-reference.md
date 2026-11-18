# Mod-tools reference — what the open-source modding world proves about Skyrim's formats

Research done 2026-11-14 (read-only: no code edited, no builds run; Ultron
owns the codebase for M12). Method: read the actual source of OpenMW,
xEdit, UESP, and NifSkope's docs, then verified every claim that matters
against Dylan's own game files in `test-data/` with read-only Python probes
(`/tmp/vhgt_audit.py`, `/tmp/door_check.py`, `/tmp/pine_probe.py`,
`/tmp/pine_probe2.py`, `/tmp/pine_probe3.py` — note `/tmp` is ephemeral;
the numbers below are the durable record).

Each finding gives: the fact, its source (file + line, or URL), what our
code does today, and what should change (or confirmation our code is
already right).

**Headline results**

1. **VHGT heights scale ×8 — SETTLED.** Our `decode_land` is missing the
   ×8 multiplier. Proven against 1,298 placed refs (median error 0.03 m
   under ×8 vs 0.95 m under ×1) and mountain trees exact to centimetres
   under ×8. This one scale error explains both of Dylan's complaints:
   ground riding up the buildings in the village AND trees floating on the
   mountain.
2. **M11's pines were the LOD trees, not the real trees.** Every pine NIF
   carries the full-detail tree as SKINNED shapes with no inline geometry
   (skipped pre-M12) plus a static low-detail version under a NiSwitchNode.
   M11 rendered only the sparse LOD cards. M12 skinning restores the full
   foliage; M13 must add switch handling or both versions will render
   superimposed.
3. **Trunk banding is vertex colors, not the texture.** The LOD trunk's
   vertex colors oscillate sharply with height (0.95 → 0.37 within 25
   units); our renderer correctly applies them. The bark texture is a
   smooth gradient with no periodic striping. The bands disappear when the
   full-detail tree is shown up close instead of the LOD.
4. **Alpha-to-coverage is the missing foliage piece** (after the skinned
   shapes return). Bevy 0.20 has first-class A2C; our code uses only
   `AlphaMode::Mask`. One implementation trap: A2C reads its cutoff from
   the material's `alpha_cutoff` field (default 0.5), not from the
   `AlphaMode` variant — the NIF threshold must be copied there.

---

## 1. VHGT height scale: ×8 — SETTLED (M13 must apply this)

### The fact
LAND VHGT heights are int8 deltas accumulated by the mixed rule (first
column accumulates down, each row accumulates across from its first
vertex), and the accumulated result — **anchor included** — is multiplied
by 8 to get game units. Our decode does the accumulation but never
multiplies by 8.

### Sources
- **OpenMW `components/esm3/loadland.cpp`, `loadLandRecordData`** (GitHub
  blob `429305401ee7486f160cb1bbd2196fc80d33dc3a`): the VHGT decode is the
  same mixed accumulation as ours, then every stored height is multiplied
  by `Land::sHeightScale` (8.0f), anchor included:
  `data.mHeights[...] = rowOffset * Land::sHeightScale` /
  `colOffset * Land::sHeightScale`. The save path confirms the inverse
  (`offsets.mHeightOffset = mLandData->mHeights[0] / sHeightScale`, diffs
  divided by sHeightScale). **Caveat:** this is OpenMW's TES3 (Morrowind)
  LAND (`sLandSize = 65`, `REAL_SIZE = 8192`). The inference to TES5 rests
  on UESP saying the formats are identical except grid size — and, now, on
  our own audit (below), which settles it for Skyrim SE directly.
- **OpenMW `components/esm/loadland.hpp`** (old esm): `LAND_SIZE = 65`,
  `REAL_SIZE = 8192`, `HEIGHT_SCALE = 8`.
- **UESP, Skyrim Mod:Mod File Format/LAND**
  (https://en.uesp.net/wiki/Skyrim_Mod:Mod_File_Format/LAND, fetched
  2026-11-14): "The final vertex Z, in game units relative to the cell
  origin, is the cumulative result multiplied by 8." Code snippet:
  `float offset = ReadFloat() * 8;` and `float value = ReadSByte()*8;`,
  with the same mixed accumulation. **But** the older prose section says
  "(assumed from Oblivion)" — the TES5 ×8 claim on that page is inherited
  from Oblivion, not independently verified there. UESP Oblivion LAND:
  "The VNML, VHGT and VCLR formats are identical to TES3, except that TES4
  land records contain 33x33 height, normal and vertex colour data points"
  and "Each value of 1 equates to a height of 8 game units."
- **xEdit carries no independent decode** (see §8): `wbDefinitionsTES5.pas`
  was derived from UESP, and xEdit treats VHGT as an opaque byte blob.
  The ×8 claim's provenance is UESP (inherited from Oblivion) + OpenMW's
  TES3 code + **our audit below, which is the first direct Skyrim SE
  proof**.

### The audit — direct proof from Dylan's Skyrim.esm
`/tmp/vhgt_audit.py` (read-only): walk Tamriel's WorldChildren group only
(XCLC grids are ambiguous across worldspaces — an early run without this
restriction was wrong; see the lesson in §7), decode all 11,186 Tamriel
LANDs under ×1 and ×8, and compare terrain height against the Z of every
placed REFR inside the Riverwood 3×3 (delta = terrain − REFR Z in metres;
refs sitting on the ground should read ≈ 0).

- Scope: 869,687 records scanned; cells 11,187; lands 11,186; refs
  250,830; **n = 1,298 REFRs** inside the 3×3.
- **×1: median +0.95 m, mean −0.15, p10 −1.89, p90 +2.33; within 0.5 m:
  226/1,298 (17%).**
- **×8: median −0.03 m, mean −0.43, p10 −2.08, p90 +0.65; within 0.5 m:
  776/1,298 (60%).**
- Smoking gun — mountain trees/rocks NW of the village (Z ≈ 2400 units):
  TreePineForestSnowL02 at Z = 2473: ×1 delta **−30.92 m** (floating 31 m
  in the air), ×8 delta **+0.02 m**. TreePineShrub01Snow: ×1 −31.03, ×8
  +0.16. TreeDeadShrubSnow: ×1 −30.92, ×8 +0.23. RockShelf01: ×1 −29.66,
  ×8 +0.01. RockM02Snow: ×1 −29.83, ×8 +0.49. TreeVineMaple01Snow: ×1
  −29.82, ×8 +0.17. Exact to centimetres under ×8.
- Named Riverwood buildings (`/tmp/door_check.py`, delta = terrain −
  REFR Z): the inn sign post (SignRiverwoodSleepingGiantInn01): ×1
  +1.90 m, ×8 **−0.01 m**. Farmhouse02: ×1 +1.50, ×8 +0.13. Farmhouse04:
  ×1 +0.22, ×8 −0.08. Farmhouse02Walkway: ×1 +1.49, ×8 −0.19.
  Farmhouse04Walkway: ×1 +0.19, ×8 −0.11. Farmhouse06: ×1 +4.72, ×8
  +3.12 (its placement origin sits below its base under both decodes —
  still better under ×8). FarmInn01 (the inn itself): ×1 +3.23, ×8 +2.60
  (same origin-below-base caveat).

**Why ×1 produces BOTH of Dylan's symptoms:** under ×1 the terrain is 8×
too flat AND its absolute height is wrong. Where true heights are negative
(the village floor, accumulated values −1..−56), ×1 terrain sits HIGHER
than the true ground (e.g. −12.8 units instead of −102.5 at the inn door),
burying building bases by ~1 m (median +0.95 m — "ground to the roof").
Where true heights are large positive (the mountain), ×1 terrain is far
BELOW the objects (trees floating ~31 m). One scale error explains both.

**Resolving the stated tension (the door arrival):** the inn door XTEL
arrival (23259.8, −44476.2, −8.3) sits +0.07 m above the ×1 terrain but
+1.35 m above the ×8 terrain, and the exterior door REFR 0x00013424
(Z = −8) sits −0.05 m under ×1 vs −1.24 m under ×8. This is the one
point that favours ×1 — and it is explained, not contradictory: the
arrival's Z (−8.3) matches the exterior door REFR's Z (−8), i.e. the
arrival was placed at **door-threshold height, on the inn's raised
entrance** (porch/foundation ~1.3 m above the surrounding true terrain),
not at ground height. A single designer-placed teleport point does not
outweigh 1,298 REFRs plus centimetre-exact mountain trees. M11's "door
matched ×1 to 0.06 m" was real but was measuring a porch, not the ground.

**Why the earlier proofs couldn't catch this:** a uniform scale error is
INVISIBLE to edge-continuity checks — scaling both sides of a shared edge
keeps them matching. The vhgt_sweep proof proves accumulation and
orientation, not scale. (`docs/formats/land.md` already records this in
its "Height scale — the ×8 question" section.)

### What our code does today
`crates/dovah-app/src/land.rs:221-242` `decode_land`: mixed accumulation
exactly as OpenMW's, but the heights are stored unscaled (no ×8 anywhere)
and treated as game units (/70 to metres).

### What should change (M13)
Multiply the accumulated height by 8 in `decode_land`, **anchor included**
(`first * 8.0`, deltas `* 8.0` — matching OpenMW's `loadLandRecordData`
and UESP's snippet). Follow-ons:
- FloorGrid / ground-height sampling use the same heights, so they inherit
  the fix; re-run the door-arrival and building-base checks after the fix
  (expect the door arrival to sit ~1.35 m above the ground — correct, it's
  a porch — and building bases to meet the ground).
- Edge-continuity tests still pass under ×8 (uniform scale preserves
  shared-edge matching); the vhgt_sweep expectations do not change shape,
  only scale.
- The village floor drops ~1 m and the mountain rises to ~65 m
  (cell (6,−12) max 65.60 m under ×8 vs 8.20 m under ×1) — the render
  should be re-screenshotted and compared against the M11 image.
- Update `docs/formats/land.md`: the ×8 question is no longer open.

---

## 2. Pine NIFs: the full tree is skinned; M11 rendered only the LOD

### The fact
Every pine NIF contains TWO trees under a NiSwitchNode: the full-detail
tree as **skinned BSTriShapes with no inline geometry** (dsize = 0,
ntri = 0, nvert = 0 — geometry lives only in the skin partitions), and a
static low-detail tree as NiTriShapes. Pre-M12, the skinned shapes were
skipped entirely, so M11 rendered only the sparse LOD cards — that, not
the alpha test, is the main reason the pines looked wispy.

### Sources
Probed from Dylan's Meshes BSAs (`/tmp/pine_probe.py`):
`meshes/landscape/trees/treePineForest01.nif` (43 blocks; the same
structure in `treePineForest02.nif` and `treePineForestSnowL02.nif`):

- [0] `BSTreeNode` (root), [3] `NiSwitchNode` with children **[4] and
  [32]**.
- Child [4] `NiNode` (children `[−1, 5, 19, 26]`): the FULL-DETAIL tree.
  [5] `TrunkBone` → branch bones [11–18]. [19] `BSTriShape` foliage:
  skin = 20, shader = 23, alpha = 25, texture
  `textures\landscape\trees\TreePineForestBranchComp.dds`, **ntri = 0,
  nvert = 0, dsize = 0**. [26] `BSTriShape` trunk: skin = 27, shader =
  30, alpha = −1 (opaque), texture `TreePineForestBarkComp.dds`, also
  **dsize = 0**.
- Child [32] `NiNode` (children `[33, 36]`): the LOD tree. [33]
  `NiTriShape` 'TreePineForest01_1:0' (trunk, static, 413 verts / 496
  tris) and [36] `NiTriShape` 'TreePineForest01_1:1' (foliage, static,
  895 verts / 632 tris, alpha 0x12ec threshold 112).
- Full-detail geometry counts (skin partitions, `/tmp/pine_probe3.py`):
  trunk partition [29]: 1 partition, **1,004 verts**; foliage partition
  [22]: 3 partitions, **1,422 verts**. The LOD has 413 + 895 = 1,308
  verts total vs the full tree's 2,426 — and far fewer, sparser foliage
  cards (632 tris vs the full foliage's partition triangles).
- SkyrimPrefs.ini corroborates the design: `bRenderSkinnedTrees=1`,
  `uiMaxSkinnedTreesToRender=20` — near trees are SKINNED full-detail
  models; the switch selects the LOD at distance.

### What our code does today
Pre-M12, `dovah-nif` skipped shapes with no inline geometry, so only the
LOD shapes rendered. M12 (in progress) assembles the skinned shapes in
bind pose, which restores the full foliage and trunk.

### What should change (M13)
**NiSwitchNode handling.** Our tree walk (`meshes()`) does not know about
switches: once M12 lands, BOTH the full-detail tree and the LOD tree will
render superimposed at every distance. M13 must pick one child per
switch — simplest correct behaviour: always render child 0 (the
full-detail child [4]; the stored default — see §3 — is child 0), and
skip the LOD child. Distance-based switching (full near, LOD far) is the
real engine's behaviour and a later optimisation, not needed for
correctness. Verify from the M13 render: pines full up close, no doubling.

---

## 3. BSTreeNode / NiSwitchNode are Unknown in dovah-nif

### The fact
Our NIF parser only treats `NiNode` and `BSFadeNode` as nodes. `BSTreeNode`
(the standard root of tree NIFs) and `NiSwitchNode` fall through to
`Block::Unknown`, so the tree walk dies at block 0 for every pine. Both
types parse cleanly as a NiNode prefix plus a short tail, so they can be
parsed as nodes.

### Sources
- Our dispatch: `crates/dovah-nif/src/lib.rs:1675-1681` — only `"NiNode"`
  and `"BSFadeNode"` map to `Block::Node`.
- Our walk: `crates/dovah-nif/src/lib.rs:986-1055` `meshes()` — only
  `Block::Node` descends into children; the fallback loop at :1032
  renders shapes the walk never reached with the identity transform.
  That fallback is why pines rendered at all in M11 (the LOD shapes'
  ancestors are all identity transforms, so identity was correct for
  them by luck).
- Layout proof (`/tmp/pine_probe2.py`, treePineForest01.nif):
  - [0] `BSTreeNode`: NiNode prefix parses (name 'TreePineForest01.nif',
    children `[2]`), **44 trailing bytes**: 11 u32s —
    `1, 5, 8, 11, 12, 13, 14, 15, 16, 17, 18` (plausibly a count + block
    refs; tail meaning not needed — blocks are size-bounded, so the
    parser can ignore the tail).
  - [3] `NiSwitchNode`: NiNode prefix parses (name 'TreePineForest01',
    children `[4, 32]`), **6 trailing bytes**: `03 00 00 00 00 00`
    (u16 = 3, u32 = 0 — consistent with switch flags + active-child
    index 0, i.e. the full-detail child is the stored default).
  - [4], [5], [11], [32] plain `NiNode`s parse with 0 trailing bytes.
- Bone-transform consequence: because the walk dies at the root,
  `node_worlds` is empty for pines, and skinning falls back to each
  bone's LOCAL transform (`lib.rs:991-993` comment). For pines this is
  correct anyway — every ancestor of the bones ([0], [3], [4], [5]) has
  translation (0,0,0) — but it is fragile: any NIF whose switch/root
  nodes carry a transform would misplace its skinned shapes.

### What our code does today
As above: both types Unknown; pines render via the identity fallback;
bone transforms fall back to local.

### What should change (M13)
Add `"BSTreeNode"` and `"NiSwitchNode"` to the node dispatch (same
`parse_node` as `BSFadeNode`, trailing bytes ignored — the block reader is
size-bounded). Then the walk reaches the bones (correct `node_worlds`)
and the switch children (enabling §2's child selection). Regression test:
parse treePineForest01.nif and assert the walk visits blocks 19 and 26
with composed (identity) transforms and that the switch node reports
children [4, 32].

**M13 correction — this section missed a block.** The BSTreeNode
root's only child is [2], a `BSMultiBoundNode`, and the switch is
block 2's child — so parsing BSTreeNode + NiSwitchNode alone
still leaves the walk dying at block 2, and all four pine shapes
render via the identity fallback (verified: `nif_tree` on the
real file showed 4 meshes before the fix). `BSMultiBoundNode`
(NiNode prefix + one i32 multibound ref) must ALSO parse as a
node. After all three, `meshes()` returns exactly the 2
full-detail shapes (1,422 + 1,004 verts).

---

## 4. Foliage alpha: threshold semantics confirmed; alpha-to-coverage is the missing piece

### The fact
NiAlphaProperty bit 9 = alpha test enable; bits 10–12 = test mode; the
threshold byte is the cutoff. Pine foliage is flags `0x12EC`, threshold
**112**, mode GREATER (test-only, no blending). Our threshold handling is
correct. What we are missing is **alpha-to-coverage**: Bevy 0.20 has it
first-class, and with MSAA it softens needle-card edges instead of
hard-discarding them — the difference between wispy and full at the
pixels that matter. (The bigger sparseness cause was §2's missing skinned
shapes; A2C is the finish on top.)

### Sources
- **NiAlphaProperty layout** (confirmed across OpenMW
  `components/nif/property.hpp` `S_AlphaProperty` comment, PyFFI NIF docs,
  CommonLibSSE `RE::NiAlphaProperty`): bit 0 = blending enable; bits 1–4
  = source blend mode; bits 5–8 = destination blend mode; bit 9 = alpha
  test enable; bits 10–12 = test mode (000 ALWAYS, 001 LESS, 010 EQUAL,
  011 LEQUAL, 100 GREATER, 101 NOTEQUAL, 110 GEQUAL, 111 NEVER); bit 13 =
  no-sorter flag. NifSkope guidance: flags 4844 (`0x12EC`) + threshold
  128 is the standard foliage alpha-test setup.
- **Branch texture alpha** (`/tmp/pine_probe.py`, PIL decode of
  `textures/landscape/trees/TreePineForestBranchComp.dds`, 4096×2048
  DXT5, 13 mips): 80.1% of pixels have alpha == 0, 6.6% in 1..111,
  **13.3% ≥ 112** (coverage at the real threshold). Each needle card is
  ~87% empty — fullness comes from card COUNT (§2), not from the texture.
- **Bevy 0.20 A2C** (verified in
  `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_pbr-0.20.1/`):
  - `src/render/material.rs:692-704` `alpha_mode_pipeline_key`: with MSAA
    on, A2C selects `MeshPipelineKey::BLEND_ALPHA_TO_COVERAGE`; with MSAA
    off it falls back to `MAY_DISCARD` (plain discard = Mask behaviour).
  - `src/render/mesh.rs:3558-3567`: the A2C pipeline sets
    `alpha_to_coverage_enabled = true`, `depth_write_enabled = true`,
    blend None, and pushes the `ALPHA_TO_COVERAGE` shader def.
  - `src/render/pbr_fragment.wesl:212-224`: with A2C, the shader sharpens
    alpha edges — `alpha = (alpha - alpha_cutoff) / max(fwidth(alpha),
    0.0001) + 0.5` (the bgolus anti-aliased alpha-test technique).
    **It reads `alpha_cutoff` from the material uniform.**
  - `src/render/pbr_functions.wesl:100-125`: with MSAA off, A2C falls back
    to Mask discard in the shader too.
  - `src/render/pbr_prepass_functions.wesl:17-30`: prepass discard for
    A2C uses `PREMULTIPLIED_ALPHA_CUTOFF = 0.05` (only near-transparent
    pixels are dropped from the depth prepass).
  - `src/render/pbr_types.wesl:63`: `ALPHA_MODE_ALPHA_TO_COVERAGE`
    flag constant.
  - **The trap** — `src/pbr_material.rs:1190-1194`: `alpha_cutoff`
    defaults to 0.5 and is overridden ONLY by `AlphaMode::Mask(c)`.
    `AlphaMode::AlphaToCoverage` carries no value, so **A2C uses
    0.5**. **M13 correction:** `StandardMaterial` has NO
    `alpha_cutoff` field to set — the 0.5 lives in the
    `StandardMaterialUniform` and only `Mask(c)` changes it. The
    "copy the threshold there" fix in this doc's §4
    recommendation does not compile in Bevy 0.20. M13 ships A2C
    at 0.5 (vs the NIF's 0.439) and judges by the render; the
    correct threshold fix, if the foliage reads too thin, is a
    texture alpha rescale at decode (alpha × 0.5/0.439), not a
    lower threshold.
  - `Msaa` default in Bevy 0.20 is `Sample4`
    (`bevy_render-0.20.1/src/view/mod.rs:248-258`).
- **Mipmap alpha coverage** (DynDOLOD / Step Mods, sheson): "for foliage
  in particular, you need to scale the alpha for mipmap coverage"
  (mipmaps lose alpha coverage as they downsample unless scaled);
  "Iteratively subtract 16 from the threshold and retest if trees are too
  skinny". Tamriel Rebuilt, "Things I learned about alphatested foliage"
  (Hemaris): near LOD uses alpha BLENDING (flags 4845 = `0x12ED`), far
  LOD uses alpha TEST (4844 = `0x12EC`) with a slightly higher threshold;
  texture filter mode `FILTER_BILERP_MIPNEAREST` looked best; the
  transparent ("ground") RGB should be close to the foliage RGB (bleed
  matters at edges).

### What our code does today
- `crates/dovah-nif/src/lib.rs:756-762` `alpha_test_cutoff`: bit 9 set →
  `Some(threshold / 255.0)` — **correct**, matches the layout above.
  `parse_alpha_property` (:2562-2571): ObjectNET + flags u16 + threshold
  u8 — correct.
- `crates/dovah-app/src/scene.rs:548-549` `real_material`: alpha-tested
  materials get `AlphaMode::Mask(cutoff)` — plain discard, no A2C.
- Our code sets no `Msaa` resource anywhere, so Bevy's `Sample4` default
  applies (desktop; **verify it actually holds on Android** — if the
  phone's MSAA is off or 2×, A2C degrades or weakens).

### What should change (M13)
For alpha-tested foliage materials (NiAlphaProperty bit 9 set, blend bit
0 clear — the `0x12EC` family): use `AlphaMode::AlphaToCoverage`.
~~and set `material.alpha_cutoff = threshold / 255.0`~~ — **M13
correction: no such field exists on `StandardMaterial` in Bevy 0.20**
(see the trap note above); A2C runs at 0.5 vs the NIF's 0.439
(112/255 for the pines). Keep `AlphaMode::Mask` as the no-MSAA fallback
path (Bevy does this automatically when MSAA is off). Keep double-sided
(the foliage shaders carry flags2 `0x10` = Double_Sided — confirmed in
§5). Do NOT lower the threshold to fake fullness: the real fix is §2's
skinned cards; the DynDOLOD "subtract 16" advice is for LOD tuning, not
for the full-detail tree.

---

## 5. Trunk banding: vertex colors, not the texture (M13 verification item)

### The fact
The striped trunks in the M11 render are the LOD trunk's **vertex
colors**, which oscillate sharply with height and are multiplied onto the
bark texture. The bark texture itself is NOT periodically banded, and the
UV tiling cannot produce stripes under Bevy's default sampler. Our
renderer applies vertex colors exactly as the real engine does (the trunk
shader has the Vertex_Colors flag set) — the banding is a symptom of
showing the LOD tree up close (§2), not a bug in our color handling.

### Sources
- **LOD trunk vertex colors oscillate with height**
  (`/tmp/pine_probe2.py`, NiTriShapeData [34] of treePineForest01.nif,
  413 verts): mean brightness per 100-unit Z bin: −100: 0.226, 0: 0.538,
  100: 0.701, 200: 0.874, 300: 0.909, 400: 0.641, 500: 0.370, 600: 0.579,
  700: 0.407, 800: 0.766, 900: 0.383. At 25-unit resolution the jumps are
  sharper still — e.g. Z~600: 0.95 → Z~625: 0.37 → Z~650: 0.70 → Z~675:
  0.38. ~8 brightness peaks over the visible trunk height.
- **Render stripe count matches**: a row-averaged vertical profile of the
  trunk in `screenshots/riverwood-m11-exterior.png` (x 860–950, y
  390–640) shows ~8–10 strong bands (brightness swinging ~50 ↔ ~70) —
  the same count as the vertex-color peaks. The chevron pattern at the
  trunk base follows the root flare's surface, as interpolated vertex
  colors would.
- **The bark texture is not banded** (`/tmp/pine_probe.py` + probe2):
  `textures/landscape/trees/TreePineForestBarkComp.dds`, 512×2048 DXT1,
  9 mips, opaque. Row-mean brightness 57.7 ± 10.1: a smooth vertical
  gradient (bright ~67–72 for rows 0–1280, dark ~44–49 for rows
  1280–2048) with broad undulations, NO periodic striping —
  autocorrelation top lags are 2–6 (local smoothness only, r ≈ 0.92–0.95).
  Row variation (std 10.1) exceeds column variation (std 3.3), i.e. the
  texture's structure is horizontal strata, but broad, not striped.
- **UV tiling cannot stripe under our sampler**: the LOD trunk's UVs span
  u[−0.070..4.363], v[−5.000..1.000]; v falls with height
  (corr(v, Z) = −0.803; slope −0.00208/unit → 4.75 tiles over the trunk).
  BUT our code sets no sampler anywhere (grep for `ImageSampler` /
  `address_mode` / `Repeat` across `crates/` returns nothing), so Bevy's
  default applies — and Bevy 0.20's default is **ClampToEdge**
  (`bevy_image-0.20.1/src/image.rs:752-753`, `#[default] ClampToEdge`;
  `ImageSamplerDescriptor::linear()` at :903-910 keeps the default address
  modes; `ImagePlugin::default()` at :181 uses `linear()`). With clamping,
  v ∈ [−5, 0) samples the v = 0 edge row — the texture contributes one
  broad dark-base → bright transition, not stripes. (Caveat for the
  future: if M13 ever sets `Repeat` for a texture class, tree trunks
  WOULD band from the texture gradient × 4.75 tiles — don't.)
- **The real engine applies these vertex colors**: all four pine shaders
  carry flags2 bit `0x20` = `SLSF2_Vertex_Colors` (full foliage
  flags2 = `0x22008031`, full trunk `0x00008021`, LOD trunk `0x00008021`,
  LOD foliage `0x22008031`). Sources for the flag: Nexus Mods,
  "BSLightingShaderProperty Basics"
  (https://forums.nexusmods.com/topic/1188259-bslightingshaderproperty-basics/):
  "Shader Flags 2 ~ Vertex_Colors ENABLED *only required if you are using
  vertex colors"; AFK Mods, "Dissecting Skyrim Meshes"
  (https://www.afkmods.com/index.php?/topic/3656-skyrim-dissecting-skyrim-meshes/):
  "The SLSF_2_Vertex_Colors option depends on the value of the
  NiTriShapeData 'Has Vertex Colors'"; NifSkope DeepWiki
  (https://deepwiki.com/niftools/nifskope/7-advanced-topics):
  "`SLSF2_Vertex_Colors` flag enables color array". Cross-checks that the
  flag decoding is right: flags1 bit `0x2` (Skinned) is set exactly on
  the two skinned shapes' shaders and not on the LOD shaders; flags2 bit
  `0x10` (Double_Sided) is set exactly on the two foliage shaders.
- **The full-detail trunk's vertex colors also oscillate, but less
  sharply** (`/tmp/pine_probe3.py`, skin partition [29], 1,004 verts):
  Z-bin means 0.132 → 0.410 → 0.709 → 0.843 → 0.917 → 0.608 → 0.478 →
  0.649 → 0.588 → 0.805 → 0.490 → ~0.52–0.60 above Z 1000, with LARGE
  within-bin spreads (e.g. Z~0: min 0.009, max 1.000, n = 450) — i.e.
  per-vertex AO varying with surface exposure, not a clean function of
  height. Prediction: the M12 full-detail trunk will read as mottled AO
  shading, not clean stripes.

### What our code does today
`crates/dovah-app/src/convert.rs:103-104` passes NIF vertex colors to
Bevy (`Mesh::ATTRIBUTE_COLOR`) whenever present, and Bevy's PBR
multiplies base color by them — **correct**, matching the real engine
(flags2 `0x20` set on the trunk shaders).

### What should change
**Nothing in the color path.** The fix is §2 (show the full-detail tree
up close via M12 skinning + switch handling). M13 verification item:
check the M12/M13 render — if the full-detail trunk still shows clean
horizontal stripes, the next hypothesis is that the real engine's tree
shader treats vertex colors differently from a plain multiply (e.g. the
`SLSF2_Tree_Anim` path), and that needs a fresh investigation; do not
"fix" it by dropping vertex colors globally (they are correct data, and
the flag is set).

### M13 verification result (2026-11-15)
**No banding** on the full-detail trunk in the pine close-up — the LOD
striping is gone. The trunk reads VERY dark (nearly black with subtle
vertical striations): full-trunk vertex color mean 0.51 with near-black
mins (`vcolor_stats`, 1,004 verts) — that darkness is DATA, not a code
bug. Whether the real game's trunk is brighter (lighting / vertex-color
handling) is an open question for the next exterior pass.

---

## 6. Foliage vertex alpha (note for M13 verification)

### The fact
The foliage shaders carry flags1 bit `0x8` (Vertex_Alpha), and the
full-detail foliage's vertex alpha varies widely (mean 109/255, min 0,
max 255 — per-vertex edge fading on the needle cards). The real engine
multiplies texture alpha by vertex alpha before the alpha test.

### Sources
- Pine shader flags (`/tmp` probe, treePineForest01.nif): foliage
  shaders [23]/[38] flags1 = `0x8040030a` / `0x80400308` (bit `0x8` set);
  trunk shaders [30]/[35] flags1 = `0x80400302` / `0x80400300` (bit `0x8`
  clear). Full foliage vertex alpha from skin partition [22]
  (`/tmp/pine_probe3.py`, 1,422 verts): mean 109.3, min 0, max 255. Full
  trunk vertex alpha is uniformly 255.
- Nexus Mods, "BSLightingShaderProperty Basics" (URL in §5): "Vertex
  Colors Alpha: Alpha channel of vertex colors controls transparency
  without the need for a NiAlphaProperty."

### What our code does today
`convert.rs:103-104` passes the full RGBA vertex color; Bevy's PBR
multiplies base color (including alpha) by it — so vertex alpha IS
applied to the alpha test, matching the real engine.

### What should change
Nothing — but M13 should verify from the render that the foliage reads
correctly (vertex alpha fading the card edges, not hollowing the cards).
If the foliage looks too thin even with the skinned shapes + A2C, the
vertex-alpha interaction is the next thing to measure (compare coverage
with and without the alpha channel).

### M13 FOLLOW-UP — this WAS the foliage problem (2026-11-15)
The M13 pine close-up rendered the full tree as bare black twigs: the
low vertex alpha (mean 0.43) fed Bevy's alpha test (PBR multiplies
vertex alpha into base alpha before the test), so most fragments died
at the 0.5 A2C cutoff. Two compounding facts: (1) the vertex alpha is
a blend-path signal — the real engine's cutout tests the TEXTURE alpha
against the NIF threshold; (2) Bevy 0.20's desktop screenshot path
renders into a **1-sample capture texture** (`sample_count: 1`,
`bevy_render-0.20.1/src/view/window/screenshot.rs:429`), so A2C
degrades to a binary 0.5 cutout in desktop screenshots — desktop
renders UNDERSTATE the foliage; the phone (Sample4 swapchain) is the
real judge. **Fix (implemented, proven by the re-render):**
`convert.rs` forces vertex color alpha to 1.0 for cutout shapes
(`alpha_cutoff.is_some()`), keeping RGB — the test then sees the
texture alpha alone. Non-cutout shapes keep vertex alpha. The
re-render shows dense needle mass instead of twigs. Regression test:
`cutout_mesh_forces_vertex_alpha_opaque` in `convert.rs`.

---

## 7. ESM layout note: interior door REFRs are not under WorldChildren

### The fact
The inn's interior door REFR 0x00013419 (whose XTEL leads outside) is
filed under the INTERIOR cell's group stack, not under Tamriel's
WorldChildren group — so a Tamriel-only scan finds no XTELs. This is
expected ESM layout, not a data error.

### Sources
`/tmp/door_check.py`: REFR 0x00013419's enclosing groups are type 0
(label `0x4c4c4543` = "CELL"), type 2, type 3, type 6 (label
0x000133c6), type 8 (label 0x000133c6) — the interior-cell block path
for cell RiverwoodSleepingGiantInn (0x000133c6). Its XTEL: destination
0x00013424, arrival (23259.8, −44476.2, −8.3), rotation (0, 0, −0.842).
Related lesson from the audit: XCLC grids are ambiguous across
worldspaces (an early audit run without the WorldChildren restriction
produced bogus LANDs — whole columns identical — because other
worldspaces' LANDs overwrote Tamriel's in the grid map). Always walk
Tamriel's WorldChildren group (label = WRLD form 0x0000003c).

### What our code does today
`dovah-esm` + the M9 loader already handle this (persistent cell by form
ID 0x00000d74, doors resolving both ways). **No change** — this is a
tooling note for future audits, recorded here so nobody re-learns it.

---

## 8. xEdit carries no independent VHGT decode (provenance note)

### The fact
xEdit's `wbDefinitionsTES5.pas` cannot settle the ×8 question: it was
derived from UESP, and xEdit treats VHGT as an opaque byte blob.

### Sources
- `wbDefinitionsTES5.pas` on GitHub (TES5Edit/TES5Edit, dev-4.1.6):
  the web view truncates at the first 1,000 lines and the LAND definition
  is far beyond it; targeted searches did not surface the LAND block.
- Secondary evidence on what xEdit does with VHGT: xEdit has a "Simple
  records LAND, NAVI, NAVM, CELL, WRLD" option (Beyond Skyrim wiki, Arcane
  University: World space synchronization), and Step Mods advises "Use
  xEdit 'Simple records' in its options to show the hex values for the
  VHGT" — i.e. xEdit shows VHGT as raw hex by default and carries NO
  height decode of its own.
- The 2012 TES5Dump project thread: "We will start by editing a file
  named wbDefinitionsTES5.pas with the definitions from
  http://www.uesp.net/wiki/Tes5Mod:Mod_File_Format" — xEdit's definitions
  were derived from UESP, so xEdit agreeing with UESP would not be
  independent evidence anyway.

### What this means
The ×8 claim's provenance is UESP (inherited from Oblivion) + OpenMW's
TES3 code. Our audit (§1) is the independent Skyrim SE proof — it does
not depend on any of these sources.

---

## 9. White flat plane above the inn sign — IDENTIFIED (M13, 2026-11-15)

### The fact
The white band/plane visible above the inn sign in the M11–M13 renders
is the **waterfall skirt/fog complex NW of the village rendering
opaque white because we have no transparent pass**, backed by the
snow-covered mountain. It is NOT a misplaced mesh and NOT an M13 code
bug.

### Evidence
- `FXWaterfallSkirtTallFront.nif` (`vcolor_stats` probe): `newFog`
  53 verts, pure white (1.00, 1.00, 1.00, 0.38), local AABB
  1196×770×3308u = 17×11×47 m; `L2_Object04` pure white alpha 0.19;
  `L2_FXWaterfallSkirtFillMesh01` pure white alpha 0.31; `L2_jet03:0`
  pure white alpha 0.76. All use `BSEffectShaderProperty` +
  `NiAlphaProperty` (blend) — designed as nearly-transparent mist,
  rendered OPAQUE by our engine (blend-only → opaque per the M11
  rule; no transparent pass yet). Skirt ref 0x000185c0 at Bevy
  (277.3, −0.8, 559.4), ~90 m NW, elevated — matches the band's
  position.
- Snow cliffs behind add whiteness: `MountainCliff03_LightSN`
  0x0004d6f2 at Bevy (239.6, 12.3, 563.2), essentially on the view
  axis; `MountainCliff02_SparseSN` 0x0004d716 at 10° right.
- The waterfall sheet itself is RULED OUT: `fallsMesh02` local
  t=(0.9, −21.0, −1027.8), top ref 0x000185c1 at Z=1202u, so the sheet
  sits at world Z ≈ 174u ≈ 2.5 m, ~1° above the horizon — far below
  the band (7–13°). Bridge01 also ruled out (36 m away, 62 m
  off-axis — north, not NW).
- The M11→M13 size growth is explained by the ×8 terrain rising to
  meet the cliffs + the M12 yaw fix rotating the effect sheets from
  edge-on toward face-on.
- Open detail: the `newFog` sheet's 47 m local Z extent vs the band's
  ~5.4° height — the sheet may be largely horizontal (its rotation
  wasn't printed). The identification stands on position + material +
  size; exact sheet orientation is a detail for the transparency
  milestone. **Fix belongs to the transparent-pass milestone, not
  M13.** Distance fog / atmospheric haze would also soften it.

---

## M13 checklist (from this research, in order) — ALL DONE 2026-11-15

1. ✅ **Apply the ×8 fix** in `decode_land` (anchor included) and
   re-prove terrain vs building bases across the village;
   re-screenshot. Proven: median 0.00 m, 60% within 0.5 m (n=1,161).
2. ✅ **Parse `BSTreeNode` + `NiSwitchNode` as nodes** in `dovah-nif`
   (§3) — plus `BSMultiBoundNode` (the research missed it; the walk
   died there until fixed) and `BSOrderedNode` (waterfall meshes
   hang under it).
3. ✅ **NiSwitchNode child selection** (§2): only the active child
   renders; inactive subtrees marked visited without collecting.
   Sweep: 262 LOD meshes suppressed, 0 regressions.
4. ✅ **A2C for foliage** (§4): `AlphaMode::AlphaToCoverage` (M13
   correction: no settable cutoff exists on `StandardMaterial` —
   A2C runs at 0.5). MSAA Sample4 holds on the phone swapchain by
   default; desktop screenshots are 1-sample (see §6 follow-up).
5. ✅ **Verify the full-detail trees**: foliage dense after the
   vertex-alpha fix (§6 follow-up); trunks unbanded but very dark
   (vertex color mean 0.51 — data, not a bug).
6. ✅ **Do NOT** change the vertex-color path, lower alpha thresholds
   to fake fullness, or set texture Repeat globally — all three
   would be wrong fixes for symptoms whose real causes are above.
   (The vertex-alpha STRIP in §6 is not a vertex-color-path change:
   it removes a blend-path signal from the cutout test only.)

## Appendix: probe scripts and key numbers

Probes (all read-only against `test-data/`; in `/tmp`, ephemeral):
`/tmp/vhgt_audit.py` (REFR-vs-terrain audit), `/tmp/door_check.py`
(door arrival + named buildings + group stack),
`/tmp/pine_probe.py` (pine NIF structure, alpha props, DDS stats),
`/tmp/pine_probe2.py` (node structure, LOD UVs, bark row profile),
`/tmp/pine_probe3.py` (skinned vertex colors).

Key numbers at a glance:

| Quantity | Value |
|---|---|
| VHGT audit, ×1 median / within 0.5 m | +0.95 m / 17% (226/1,298) |
| VHGT audit, ×8 median / within 0.5 m | −0.03 m / 60% (776/1,298) |
| Mountain TreePineForestSnowL02, ×1 / ×8 | −30.92 m / +0.02 m |
| Inn sign post, ×1 / ×8 | +1.90 m / −0.01 m |
| Door arrival above terrain, ×1 / ×8 | +0.07 m / +1.35 m (porch) |
| Pine foliage alpha flags / threshold | 0x12EC / 112 (GREATER) |
| Branch texture coverage at threshold 112 | 13.3% (80.1% alpha == 0) |
| Bark texture | 512×2048 DXT1, 9 mips, opaque, smooth gradient |
| LOD trunk UV v range / tiles | [−5.00..1.00] / 4.75× (clamped in our renderer) |
| LOD trunk vertex colors | oscillate 0.22–0.91 by height (~8 peaks) |
| Full trunk verts / full foliage verts | 1,004 / 1,422 (skinned, M12) |
| LOD trunk verts / LOD foliage verts | 413 / 895 (what M11 rendered) |
| Bevy default sampler | ClampToEdge (no Repeat set anywhere in our code) |
| Bevy Msaa default | Sample4 (verify on Android) |
