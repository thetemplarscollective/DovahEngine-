# Loading Performance — Diagnosis (Milestone 3)

Read-only diagnosis, 2026-11-17 (two passes merged: the first pass
wrote the bulk of this doc; a second pass verified it against the
code, corrected two claims — the ESM-parse RSS and the OnEnter
freeze — and added RC10, the BSA read mechanism, and the per-archive
counts). Sources: the v0.4.13 phone logs
(`gameplay/dovahengine-startup-v0413-current-20261117.log`,
`...-prev-20261117.log`), `crates/dovah-app/src/{assets,lib,scene,convert,vtxt}.rs`,
`crates/dovah-{bsa,dds,esm}/src/lib.rs`, and the ChatGPT brief
(`docs/chatgpt-rendering-presentation-brief-20261117.md`, Phase 4).
No code was run to produce this; every number below is either quoted
from the logs or derived from them. Where a number must be measured
before the fix, it is named as a measurement, not guessed.

## 0. The measured timeline

Two v0.4.13 runs on the S25 Ultra. All times are wall seconds from
app launch (current run) or from the door transition (both runs).

### Launch → Sleeping Giant Inn walkable

| Stage | Prev run | Current run |
|---|---|---|
| ESM parse (worker thread, 920,181 records) | 17.65 s | 11.82 s |
| BSA discovery + index (worker, 18 archives, 177,748 files) | 2.47 s | 1.57 s |
| Strings load (worker) | 0.17 s | 0.11 s |
| Cell resolution (worker) | 1.90 s | 1.35 s |
| **Objects phase** (437 objects) | **51.92 s** | **81.50 s** |
| Map bake (once per app run; ~2.0 s of work) | 6.44 s | 6.22 s |
| Finish (floor + obstacle grids) | 0.04 s | 0.02 s |
| Ruffle HUD prepare + ingame enter | ~0.3 s | ~0.3 s |
| OnEnter(InGame) indexes (catalog 1.2–1.35 s + rest) | ~2.4 s | ~2.3 s |
| **Launch → walkable** | **~81 s** | **~103 s** |

### Door transition → Riverwood exterior walkable

| Stage | Prev run | Current run |
|---|---|---|
| Worker `load_scene` (cell resolution) | 1.64 s | 1.70 s |
| **Objects phase** (1,107 objects) | **80.45 s** | **90.37 s** |
| Dressing: terrain bake (9 LAND cells) | 9.10 s | 8.22 s |
| Dressing: water + sky | 0.62 s | 0.52 s |
| Map bake | skipped | skipped |
| Finish | 0.20 s | 0.18 s |
| **Door → walkable** | **~92 s** | **~101 s** |

### Door transition → Sven's house walkable (current run, 143 objects)

Worker `load_scene` 2.14 s; **Objects phase 34.16 s**; Finish 0.01 s;
**door → walkable ~36.5 s**.

### What the numbers say

1. **The Objects phase is 85–90% of every load.** Everything else
   combined (ESM parse, BSA index, terrain, water, map bake, finish,
   post-load indexes) is 10–15%.
2. **Per-object wall time is highest in the smallest scene:**
   Sven's 239 ms/object (159 texture decodes / 143 objects),
   inn 119–186 ms/object (295 decodes / 437 objects),
   exterior 73–82 ms/object (305 decodes / 1,107 objects).
   Wall time tracks **texture decodes per object**, not object
   count — the cost is first-use asset work, not entity spawning.
3. **Identical work varies 57% between runs** (inn Objects 51.9 s
   vs 81.5 s, same 437 objects, same 295 decodes). The build is
   CPU-bound and thermally sensitive (the current run is the
   second consecutive run; the phone was warm). The fix must cut
   total CPU work, not just re-schedule it — and measurements
   must be taken on a cool phone with repeat runs.
4. **Frames during the build run 200 ms–2.5 s.** The Ruffle perf
   windows logged during the exterior build show 2–26 movie steps
   per 5 s window (1 step max per frame), i.e. 0.4–5 frames/s.
   The loading screen is the only thing rendering; the frames
   are slow because of the spawn work inside them (see RC3).
5. **The frame rate after loading (7–16 fps) is a separate
   problem** (render cost + Ruffle capture), owned by the
   frame-time work, not this milestone — except where it feeds
   back into load time (RC3).

## 1. Root causes, ranked

### RC1 — No cross-scene asset persistence (the single biggest win)

**Mechanism.** The scene builder has caches — `mesh_cache`
(parsed NIF → sub-meshes), `material_cache` (texture set →
material), `tex_cache` (terrain textures), `collision_cache`,
`door_anim_cache` — but they all live in `SpawnProgress`
(`scene.rs:640-718`), and `begin_spawning` (`scene.rs:2143`)
creates a **fresh `SpawnProgress` on every scene load**. Every
door transition re-extracts, re-parses, and re-decodes
everything. Bevy's asset stores drop the old scene's GPU
assets too: the transition despawns all `SceneEntity` entities
(`lib.rs:1605`), the strong handles drop, and Bevy's refcount
GC frees the meshes/materials/textures. There is no explicit
asset clearing and no persistence anywhere.

**Proof from the logs.** The exterior build decodes 305 textures
even though it shares most of its texture set with the inn
(wood, stone, dirt, trees) — if textures persisted, the count
would be near zero for shared assets. The inn decodes 295, the
exterior 305, Sven's 159: every scene pays the full decode
cost of its own texture set.

**Fix.** Move the caches out of `SpawnProgress` into a
persistent resource that survives scene loads: parsed-NIF
cache (keyed by path), decoded-image cache (keyed by texture
path — note there is currently **no image cache at all**; see
RC2), material cache, collision cache, door-anim cache, and
baked terrain textures (keyed by cell + layer set). Bound it
with LRU eviction and a hard cap (see §3). Bevy handles for
cached assets must be held by the cache resource itself, or
Bevy GC will free the GPU copies out from under it.

**Expected impact.** Door transitions drop from ~90 s toward
the cost of entity spawn + floor rasterization for
already-decoded assets — plausibly 10–20 s on repeat visits
(4–9×), and a large improvement even on first visits to a new
cell, since a new cell shares most textures/NIFs with the one
just left. This is the single biggest win in the milestone.

### RC2 — First-use asset work is single-threaded, on the main thread

**Mechanism.** The first time a scene uses a NIF, `spawn_nif`
(`scene.rs:1367`) does, all on the main thread inside the
spawn budget: BSA extract (LZ4 decompress of the NIF bytes) →
`NifFile::from_bytes` (full parse, incl. skinning, controllers,
collision shapes) → per-sub-mesh `convert::nif_mesh_to_bevy`
→ per-material `real_material` (`scene.rs:968`), which reads
and decodes the diffuse DDS, the gradient DDS (fog), and the
normal-map DDS. `dds_to_bevy_image` (`convert.rs:223`) decodes
**every mip level to RGBA8 on the CPU** through `bcn_decode`
(single-threaded, no SIMD, no rayon anywhere in the DDS path);
normal maps add a per-texel X-invert + Z-reconstruction pass
over the same data. The material cache dedupes only exact
(diffuse, gradient, normal, flags) tuples — two materials
sharing a diffuse texture but differing in normal/alpha each
re-read and re-decode that diffuse DDS, because there is no
image-by-path cache.

**Scale.** ~200–300 expensive first-use objects per scene
(≈ the unique-NIF count; the exact number is not logged —
see §4), each carrying 1–3 DDS decodes. The ~800–900
remaining objects are cache hits (entity spawn + floor
rasterization only) and cheap. The Objects phase is therefore
bimodal: a few hundred slow objects dominate, not the object
count. That is why the 8 ms budget does not bound the wall
time (RC3).

**Fix.** (a) Decode/parse on a worker thread pool: the S25
Ultra has 8 cores and the build uses 1. Move NIF parsing and
DDS decoding to workers; the main thread only creates Bevy
assets from decoded data and spawns entities. (b) Add an
image-by-path cache (`HashMap<String, Handle<Image>>`) so a
diffuse texture decoded for one material is reused by every
other material that references it. (c) Consider keeping DDS
data compressed on the GPU (upload BCn blocks directly as a
compressed `TextureFormat` instead of decoding to RGBA8) —
this removes the decode cost entirely for the GPU copy, at
the cost of a larger GPU upload; it needs a format-support
check on Adreno (BC1–BC3 are widely supported; BC7 is not
universal on mobile — verify before relying on it).

**Expected impact.** 3–6× on the first-use cost (the dominant
cost on a first visit), from parallelism alone. Combined
with RC1, first-visit loads fall substantially and repeat
visits fall to near zero asset work.

### RC3 — The chunked build's wall time is frame-time-bound

**Mechanism.** `spawn_chunk` (`scene.rs:2195`) runs once per
frame and spawns objects until 8 ms elapse — but the budget
is checked **between** objects (`SPAWN_CHUNK_BUDGET`,
`scene.rs:872`, `Duration::from_millis(8)`), so a single
expensive object (a tree NIF + textures) overruns it; the
code comments say so explicitly. Worse, every frame during
the build also pays the Ruffle movie step (~10 ms run +
~6–13 ms render + ~4–50 ms capture) and the loading-screen
render, while the HUD itself is invisible behind the loading
screen. Frames during the build run 200 ms–2.5 s (§0.4), so
the build wall time is roughly the total CPU work *plus* a
large per-frame overhead multiplied by the frame count.

**Fix.** (a) Pause the Ruffle movie during `Spawning`. Verified
second pass: `ruffle_hud_render_system` (`ruffle.rs:406`,
`RenderSystems::PrepareAssets`) has **no `run_if` state gate** —
the main-world systems are gated to InGame (`ruffle.rs:373/381/
385/389`) but the render-world stepper is not — and the captured
frames are provably wasted during Spawning, because
`upload_ruffle_hud_frame` (`ruffle.rs:385`) *is* InGame-gated, so
nothing ever uploads them. The capture also includes the
GPU-drain stall (`device.poll(Wait)` blocks until all submitted
GPU work finishes — see `docs/ruffle-gpu-compositing-diagnosis.md`).
The render system already returns early when `HudState.enabled`
is false, so the fix is either to drive `enabled = false` while
in Loading/Spawning or to extract the app state into the render
world and gate on it. This closes open question 3 (below): the
HUD image cannot be visible during Spawning, since nothing
uploads it there. (b) Move parse/decode off the main thread
(RC2), so per-frame spawn work is mostly entity creation and
the 8 ms budget starts to mean something. (c) Only after (b),
consider raising the budget modestly (e.g. 16 ms) — today a
single object can exceed 1 s, so raising the budget now would
not bound frame time at all.

**Expected impact.** Removes the per-frame overhead multiplier;
build wall time approaches the raw CPU work. This is an
enabler for RC1/RC2, not a standalone fix.

### RC4 — Terrain bake is single-threaded per-texel CPU blending

**Mechanism.** `bake_terrain_texture` (`vtxt.rs:152`) bakes
each LAND cell into a 1024×1024 texture on the CPU:
1,048,576 texels, each doing a bilinear base sample plus one
bilinear opacity sample + one bilinear texture sample per
VTXT layer (up to ~8 layers/quadrant), all in scalar Rust.
Measured: 8–9 s for 9 cells, ~1 s/cell. The layer textures
are cached across cells within a build (`tex_cache`), so the
cost is the blend loop itself, not the decodes. The baked
texture is a Bevy asset that is GC'd on transition, so the
exterior re-bakes all 9 cells on every entry.

**Fix.** (a) Parallelize the blend loop across rows (rayon or
a small thread pool — the loop is embarrassingly parallel).
(b) Cache baked terrain textures across visits (RC1's
persistent cache, keyed by cell + layer set). (c) Longer
term: move the blend to the GPU (a compute shader, or splat
the layers as separate texture binds with a blend shader
instead of baking).

**Expected impact.** Dressing 8–9 s → ~1–2 s (parallel) or
~0 s (cached).

### RC5 — ESM parse (12–18 s) + BSA index (1.6–2.5 s) on every launch

**Mechanism.** `load_first` (`lib.rs:1048`) runs on a worker
thread: `Plugin::open` reads the whole 250 MB `Skyrim.esm`
into memory and parses all 920,181 records into owned
`Record`/`Group` structs (`dovah-esm/src/lib.rs:307` —
sequential, single-threaded). RSS across the parse window
rises from ~169 MB to ~1,094 MB, but Bevy init (~537 MB at
`app.run()`) happens inside the same window, so the parse
itself contributes roughly **0.5 GB**, not ~0.9 GB (an earlier
draft of this doc double-counted the Bevy init). Then
`AssetProvider::new` (`assets.rs:73`) opens all 18 BSAs and
indexes 177,748 files. The index cost is not the `normalize()`
+ `HashMap` insert per file — it is the read pattern:
`BsaArchive::from_reader` (`dovah-bsa/src/lib.rs:131`) reads
the directory with **one `read_exact` per 16-byte file record**
(plus a seek per folder — Animations alone has 1,673 folders),
on an unbuffered `File`, through the phone's FUSE-backed
`/storage/emulated/0/Download`. ~177k tiny syscalls is the
1.6–2.5 s. Both run once per launch — door transitions
reuse the parsed plugin — so this gates the first load only.

Per-archive file counts (read from the BSA headers in
test-data; Sounds + Voices_en0 are not on the VM, so their
counts are inferred): Animations 8,920; Interface 336;
Meshes0 19,338; Meshes1 14,127; Misc 21,047; Patch 901;
Shaders 1; Textures0 5,889; Textures1 885; Textures2 998;
Textures3 1,844; Textures4 1,796; Textures5 5,965; Textures6
8,941; Textures7 5,416; Textures8 94 — 96,498 across those 16.
The phone indexes 177,748, so **~81k indexed files are audio
(Sounds + Voices_en0) that the engine never reads** — it has
no audio system yet. The 16 local archives total ~9.9 GB on
disk (~13 GB with audio).

**Fix.** (a) Persistent BSA index: serialize the normalized
path → (archive, stored path) map to disk (beside the saves
or in the app's cache dir); validate by archive file
mtime + size; rebuild only when the data folder changes.
177k entries serialize to a few MB and load in well under a
second. (b) Persistent ESM record-offset index: parse record
headers once, store form-ID → file offset (+ type), and
lazily parse individual records on demand from a memory-mapped
or pread file. This is the bigger change; it also cuts the
~0.5 GB parse peak. (c) Short term, without persistence:
**buffered bulk reads** of each archive's directory block
(one or a few reads, parsed from memory) — this alone
probably captures most of the 1.6–2.5 s for a small change —
and parallelize the ESM parse across top-level groups (groups
are independent record streams) — worth ~2–3× on 8 cores.
(d) Optionally skip the audio archives entirely until the
audio milestone — but only with an explicit decision, since
it changes what `read_file` can find.

**Expected impact.** Startup data-ready 15–22 s → ~3–5 s.
Launch-only; does not help door transitions.

### RC6 — The archive priority bug (correctness + a perf trap)

**Mechanism.** `AssetProvider::new` (`assets.rs:73`) opens
archives in **alphabetical** order and indexes first-wins
(`or_insert_with`), with the comment admitting "the game's
load order is more subtle; this is M6's approximation".
`Skyrim_-_Interface.bsa` sorts before `Skyrim_-_Patch.bsa`,
so for every file present in both, `read_file` returns the
**older** Interface copy — the game loads Patch's. The
targeted fix, `read_file_latest` (`assets.rs:184`), scans
archives in reverse order, but it does so by calling
`archive.list()` (which allocates a `Vec` of every stored
path) and `normalize()`ing **every** path until it finds a
match — O(177k) string allocations per call. It is only
called twice today (hudmenu.gfx + fonts_en.swf, `ruffle.rs:
442,451`), so the cost is small, but the general bug is live
for every other file read through `read_file`, and the scan
is a footgun for any future caller.

**Fix.** Build the index last-wins in true load order (Patch
above Interface above the base archives — the load order is
the plugin order: Skyrim, Update, Dawnguard, HearthFires,
Dragonborn, then Patch), or store a per-file override list.
Then delete the `read_file_latest` scan and route all reads
through the one correct index. The Patch-vs-Interface overlap
set should be enumerated once (a cheap read-only probe for
the build phase — it was not enumerated here, to avoid
running code against the builders' tree).

**Expected impact.** Correctness (the right assets load
everywhere — this may itself fix some of the v0.4.13 visual
defects if any are stale-asset issues); small direct perf
win; removes the footgun.

### RC7 — OnEnter(InGame) does ~2 s of synchronous indexing after the loading screen drops (first entry only)

**Mechanism.** The `OnEnter(AppState::InGame)` chain
(`lib.rs:1518`) runs 21 systems, chained, on the main thread:
`despawn_loading_screen` first, then `build_catalog`
(1.22–1.35 s — 7,355 item defs, 3,075 leveled lists),
`build_interactables` (~0.35 s), `build_dialogue_index`
(~0.15 s), `build_magic_index` (~0.25 s), `build_skill_index`
(~0.27 s), quest index (~0.03 s). **Correction (second pass):
the ~2 s cost is once per app run, not once per scene.** The
catalog, dialogue, quest, magic, and skill builders all carry
a `built` guard and return early on later entries
(`inventory.rs:1508`, `dialogue.rs:527`, `pause.rs:659`,
`character.rs:1392/1413`) — the logs confirm it: the index
lines appear only after the first ingame enter. What runs on
every transition is `build_interactables` (~0.2–0.6 s),
`build_talkable_npcs` (re-indexes per scene), and
`build_combatants` (which itself does a full-plugin scan —
see RC10). The loading screen is already down when the
first-entry chain blocks the main thread, so the player gets
a ~2 s freeze on the first frame of the first scene only.

**Fix.** Build the catalog + indexes on the worker thread
during `Loading`/`Spawning` (they need only the parsed plugin
+ strings, not the scene), or split them across frames with
the same budget mechanism as the scene build. The catalog in
particular does not depend on the cell — build it once per
app run, not once per scene.

**Expected impact.** Removes the ~2 s first-frame freeze on
the first scene entry. (An earlier draft of this doc said
"every scene entry" — wrong; see the correction above.)

### RC8 — Map bake: 6.2 s wall for ~2 s of work, on the first load

**Mechanism.** The world-map bake (11,186 cells: collect
~0.2–0.3 s, decode ~0.3 s, sample ~1.2 s, shade ~0.3 s) runs
in the `MapBake` phase, chunked at the 8 ms budget — so its
wall time is 3× its work, because frames during the bake are
slow. It runs once per app run (skipped on later transitions),
so it costs 6 s of the first load only.

**Fix.** Bake on the worker thread (it needs only the plugin
+ strings; only the final image needs the main thread), or
raise its budget — it is pure CPU work with no ANR risk on a
worker. Alternatively, bake it lazily on first opening the
map instead of blocking the first scene load.

**Expected impact.** First load −6 s.

### RC9 (micro) — `read_file` stats the filesystem on every lookup

**Mechanism.** Every `read_file` call first stats a loose
file (`self.data_dir.join(...)` + `is_file()`, `assets.rs:
141`) — a FUSE stat on Android — before consulting the
archive index. ~500 reads per scene (cache misses only) at
~0.1–1 ms each.

**Fix.** Enumerate loose files once at startup into a set, or
only check loose files when the archive index misses.
Expected impact: ~0.1–0.5 s per scene. Do it while touching
`AssetProvider` for RC6; not worth a standalone change.

### Secondary: per-placement floor-grid rasterization

On mesh-cache hits, `spawn_nif` still transforms and
rasterizes **every triangle** of the mesh into the floor grid
for that placement (`scene.rs:1283-1290`), plus the obstacle
grid. For the exterior (2,216 sub-meshes) this is on the
order of a million triangle transforms on the main thread.
It is not the dominant cost (texture decode is), but it is
real, and it is per-placement work that the mesh cache does
not help. Directions: skip rasterization for meshes whose
triangles cannot affect the walkable band, or rasterize on
the worker thread per placement and merge the grids.

### RC10 — The plugin is re-indexed on every scene load (and scanned per frame)

Added in the second pass; numbered RC10 for that reason — by
impact it ranks about fifth. **Mechanism.** `load_cell`
(`cell.rs:804`) and `load_exterior` (`cell.rs:834`) each
rebuild a `HashMap<u32, &Record>` of **all 920,181 records**
(`for record in plugin.all_records()`), then rebuild
`CellIndex` (`cell.rs:439`, a full group-tree walk), `NpcIndex`
(`npc.rs:132`), and `TamrielCells` (`cell.rs:492`, another full
walk, exteriors only) — plus `find_cell` (`cell.rs:536`), which
does yet another `all_records()` + linear scan.
`all_records()` (`dovah-esm/src/lib.rs:347`) allocates a fresh
920k-element `Vec` on every call. That is the 1.3–2.0 s worker
cost on every door transition, and it is pure waste: the plugin
is immutable after parse, so these indexes never change.

The same missing index leaks into frame time.
`Plugin::find` (`dovah-esm/src/lib.rs:356`) is `all_records()`
+ a linear scan, and the crosshair resolver calls it **every
frame** the player aims at a door: `resolve_crosshair` →
`record_display_name` / `door_destination_name` → `plugin.find`
(`ruffle.rs:721/570/596`), with `drive_ruffle_hud_state`
running every frame InGame (`ruffle.rs:373`). A 920k-record
scan + a ~7 MB `Vec` allocation per frame, for a name lookup.
`build_combatants` (`combat.rs:397`) also does a full
`all_records()` scan on every ingame enter, to find the
scene's handful of NPC_/CLAS/RACE records.

**Fix.** Build the indexes once — one `by_form_id` map, one
`CellIndex`, one `TamrielCells`, one `NpcIndex` — in
`load_first` (or lazily on first use) and store them alongside
the parsed plugin in a shared struct the worker and the main
thread both read. Route `Plugin::find` through the map
(or delete it in favour of the map). The once-per-launch index
builds (`build_catalog`, dialogue, quest, magic, skill — 2.3 s
total, each doing 1–3 `all_records()` scans) get faster for
free from the same index.

**Expected impact.** Most of the 1.3–2.0 s per transition,
plus the per-frame crosshair scan (a frame-time win as well
as a loading win). This is the cheapest fix in the milestone
relative to its return — a few dozen lines, no new threads,
no cache-lifetime design.

## 2. The fix, in order

| # | Fix | Attacks | Expected impact |
|---|---|---|---|
| 1 | Persistent asset cache (RC1): mesh / image / material / collision / door-anim / baked-terrain caches survive scene loads, LRU-bounded | Re-decode on every door transition | Door transitions ~90 s → ~10–20 s on repeat visits; large gain on first visits too |
| 2 | Worker-thread NIF parse + parallel DDS decode (RC2) + image-by-path cache | Single-threaded first-use cost | 3–6× on first-visit load cost |
| 3 | Pause Ruffle during Spawning; bound per-frame work (RC3) | Frame-time overhead during the build | Build wall time → raw CPU work |
| 4 | Parallel terrain bake + baked-terrain cache (RC4) | Dressing 8–9 s | → ~1–2 s or ~0 s |
| 5 | Build the plugin indexes once (RC10): `by_form_id` / `CellIndex` / `TamrielCells` / `NpcIndex` shared across loads; route `find` through the map | 1.3–2.0 s per transition + per-frame crosshair scan | Most of the worker time per transition; per-frame scan gone |
| 6 | Persistent BSA index + ESM offset index (RC5) | Launch-only 14–20 s | Startup data-ready → ~3–5 s |
| 7 | Correct archive override priority (RC6) | Stale assets + scan footgun | Correctness; small perf win |
| 8 | Worker-thread catalog + indexes (RC7) | ~2 s first-frame freeze, first entry | Freeze gone |
| 9 | Worker-thread map bake (RC8) | 6 s first load | First load −6 s |
| 10 | Loose-file set at startup (RC9) | Per-read stats | ~0.1–0.5 s per scene |

**The single biggest win is #1, the persistent asset cache.**
It is also the largest change (cache ownership moves out of
`SpawnProgress`, Bevy handles must be held by the cache, and
eviction must be safe). Do #1 and #2 together: #2 makes the
first visit fast, #1 makes every later visit nearly free.
#3 is cheap and should land first (it is a few lines and it
makes every other measurement cleaner).

## 3. What must NOT happen

- **No whole-game preload.** The data folder holds 177,748
  files across 18 archives; the decoded set is tens of GB.
  The persistent cache MUST be bounded (suggested cap:
  512 MB–1 GB of decoded assets, LRU by scene, evicting the
  least-recently-used scene's assets first) and must never
  decode anything the current or adjacent scene does not
  reference. RSS already peaks at ~1.2 GB during load; the
  phone has headroom, but an unbounded cache will find the
  limit.
- **No ANR.** Android kills an app that does not respond for
  5 s; that is why the 8 ms budget exists. All heavy work
  goes to worker threads; the main thread stays chunked.
  Do NOT raise the spawn budget until per-object main-thread
  work is bounded (today a single object can exceed 1 s —
  raising the budget now would not bound frame time).
- **Do not change what spawns.** The chunked build must stay
  outcome-identical to the sync build (the existing
  invariant: same order, same per-object work, different
  timing). Verify with the existing regression renders +
  pixel diffs, not md5.
- **Do not lose the phase-boundary logs or the loading-screen
  progress.** The phone's startup log is the only diagnostic
  for load performance; every phase must keep its one
  boundary line, and the progress text must keep counting.
- **Do not decode on the render thread.** GPU upload stays in
  the render world; decode stays on workers/main. (The
  current `RENDER_WORLD` image usage already drops the CPU
  copy after upload — keep that.)
- **Do not break the `SpawnBudget` test hook** (the
  multi-frame test sets the budget to zero so every frame
  spawns exactly one object).

## 4. What to measure before/after

Measure each transition type separately — they have
different bottlenecks. Use the phone's startup log (the
phase lines already exist) plus the new counters below.
Take repeat runs on a cool phone; the same build varies
57% run to run (§0.3).

**Add to the log first (cheap, makes everything else
measurable):**
- Unique NIFs parsed per scene (`mesh_cache.len()` at
  build-done) and unique materials (`material_cache.len()`).
- DDS decodes by format (BC1/BC3/BC5/BC7) and size bucket.
- Per-stage time inside the Objects phase: BSA extract vs
  NIF parse vs DDS decode vs mesh convert vs rasterize vs
  entity spawn (a timing probe on desktop, or coarse
  accumulators in `spawn_object` logged at build-done).
- Ruffle step cost during `Spawning` (run/render/capture
  while the loading screen is up).

**Before/after, per transition:**
- Startup: time to first frame; ESM parse; BSA index;
  strings; cell resolution; time to data-ready.
- First-area load (inn): Objects / Dressing / MapBake /
  Finish; textures decoded; unique NIFs; OnEnter(InGame)
  hitch; RSS peak.
- Door transition (inn→exterior, exterior→Sven's,
  Sven's→exterior): worker `load_scene` time; Objects /
  Dressing / Finish; textures decoded (should drop to
  near-zero for shared assets after fix #1); ingame-enter
  hitch; RSS peak; cache size after the scene.
- Per-unit costs: DDS decode ms by format+size; NIF parse
  ms by file size; BSA extract ms by file size; terrain
  bake ms/cell.

**Acceptance bar for the milestone:** door transitions under
~20 s, first-area load under ~30 s, startup data-ready under
~10 s — with no regression in what spawns, no ANR, and RSS
peak no worse than today's ~1.2 GB.

## 5. Open questions for the build phase

1. Per-texture DDS decode cost by format/size on the phone
   CPU (needs a timing probe; the log only gives the total).
2. The Patch-vs-Interface overlap set (which files the
   priority bug actually affects beyond the two HUD files).
   A one-line collision counter on the `or_insert_with` in
   `AssetProvider::new` enumerates it.
3. ~~Whether the Ruffle HUD image node is visible during
   `Spawning` (decides whether pausing the movie is safe).~~
   **Closed in the second pass** — see RC3: the upload is
   InGame-gated, so nothing the movie renders during
   Spawning can be visible; pausing the movie is safe.
4. BCn compressed-texture upload support on Adreno 830
   (decides whether the GPU-compressed-texture path in RC2
   is viable). Note: BC support is a desktop format and is
   typically *absent* on mobile GPUs — expect "no", and
   treat the decode as unavoidable until proven otherwise.
5. The exact unique-NIF count per scene (add the counter;
   the estimate here is ~200–300 from the texture counts).
6. What share of ESM records are zlib-compressed, and how
   much of the 12–18 s parse is decompression vs allocation?
   A timed breakdown in `Plugin::from_bytes` decides between
   lazy parsing and a serialized cache (RC5).
7. Per-object CPU vs per-frame overhead during Spawning —
   the logs cannot split them. Needs the `spawn_chunk`
   instrumentation (§4) or the HUD-off experiment: run the
   phone with the Ruffle HUD off (the Bevy fallback exists;
   no code change) and compare the exterior door-transition
   time — that isolates the whole Ruffle cost in one number.
8. Where the ±30% run-to-run variance comes from. Thermal
   throttling is the leading hypothesis (the slower run was
   the second launch, warm phone). Two back-to-back runs
   with a cool-down between them would test it.
