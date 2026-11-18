# Ruffle GPU Compositing + the ~40 ms Frame Cost — Diagnosis (M6)

Read-only research, 2026-11-17. Sources: the v0.4.13 phone logs
(`gameplay/dovahengine-startup-v0413-{current,prev}-20261117.log`),
`crates/dovah-app/src/ruffle.rs`, `crates/dovah-app/src/perf.rs`,
`crates/ruffle-harness/src/runner.rs`, vendored Ruffle 0.7.1
(`vendor/ruffle/render/wgpu/src/{backend,target,utils}.rs`),
`crates/dovah-app/src/scene.rs`, `Cargo.lock`.

## Verdict

**GPU compositing is feasible with NO new Ruffle patch.** The texture is
already sampleable, a public handle exists, and the queue order gives the
synchronization for free. One optional one-line patch drops the per-frame
staging copy. Expected saving: **~30 ms/frame early in a scene,
~5–10 ms/frame at steady state** — worth doing first, but it does NOT by
itself reach 30 fps. The remaining ~40 ms is the 3D render (draw calls +
fragment cost), and that needs its own fix after the compositing lands.

## The measured numbers (paired windows, v0.4.13 phone logs)

`perf:` measures Bevy virtual-Time deltas (clamped at 250 ms — the
`max 250.0ms` values ARE the clamp). `perf ruffle:` measures, per movie
step: `run` (AS2, CPU), `render` (CPU encode + submit only — the GPU
rasterization time is NOT in this number), `capture` (readback). One
step per frame (the v0.4.11 1-step cap binds at these frame rates).

| Scene | Frame | run | render | capture | Ruffle total | Non-Ruffle |
|---|---|---|---|---|---|---|
| Inn, early (curr w2–5) | 97–99 ms | ~9.5 | ~7 | ~32 | ~48 ms | **~49 ms** |
| Exterior, late (prev w35–37) | 62.5–63 ms | ~11 | ~6.5 | ~4 | ~22 ms | **~40 ms** |
| Exterior, early (prev w2–5) | 113–126 ms | ~10 | ~8 | ~37–49 | ~55–67 ms | ~55–60 ms |

Two facts drive everything below:

1. **Capture is not a constant.** It is 24–49 ms early in a scene and
   3.5–6 ms late in a scene, while the frame stays ~62 ms. A pure
   readback would not vary like that.
2. **The frame rate improves within a run** (prev: 8 fps → 16 fps) as
   pipeline/bind-group warm-up fades — then plateaus at ~62 ms.

## Part 1 — Ruffle GPU compositing

### The current path (four transfers per frame)

1. `ruffle_hud_render_system` (Render, `PrepareAssets`) calls
   `harness.step()` — `run` (AS2) then `render` (encode + `queue.submit`
   on the shared device).
2. `harness.capture()` → `WgpuRenderBackend<TextureTarget>::capture_frame`
   (`backend.rs:202`) → `buffer_to_image` → `capture_image`
   (`utils.rs:131`): `map_async(Read)` + **`device.poll(PollType::Wait
   { submission_index: None })`** + row-depad copy + per-pixel
   `unmultiply_alpha_rgba` of 1280×720×4 = 3.7 MB.
3. The `RgbaImage` crosses the render→main mailbox (`Arc<Mutex>`).
4. `upload_ruffle_hud_frame` (main-world Update) replaces
   `image.data`, so Bevy re-uploads the whole 3.7 MB texture via
   `queue.write_texture` (`bevy_render` `gpu_image.rs:84`) — a second
   full transfer, in the opposite direction, **unmeasured**.
5. A fullscreen `ImageNode` displays it (one frame of latency).

### The key insight: capture is a GPU-drain stall, not a readback cost

`poll(Wait)` with `submission_index: None` blocks until the GPU finishes
**all** submitted work on the device — not just Ruffle's. At the moment
it runs (PrepareAssets, before this frame's graph submits), the
outstanding work is:

- Ruffle's own rasterization for this step (submitted moments before;
  its GPU time is NOT in `render_ms`, which measures CPU encode only),
- the **previous frame's entire Bevy render graph** (submitted at the
  end of the previous frame).

So the "capture" number is mostly *waiting for the GPU to drain*. The
pure CPU readback work (map + 3.7 MB copy + unmultiply) is ~2–5 ms —
and the late-scene capture average (3.5–6 ms) is the closest measurement
of it. The 24–49 ms early-scene values are the GPU being busy (first-
frame pipeline compiles, texture uploads, shadow maps).

Consequences:

- Removing the readback removes the stall, but the GPU work itself
  (Ruffle raster + 3D scene) still has to execute — it just stops
  blocking the CPU. If the GPU is the bottleneck, the CPU will block
  at present acquire (FIFO) instead. The saving is real but bounded
  (see the estimate).
- The capture number also **contaminates the diagnosis**: while the
  stall exists, no other cost can be measured cleanly. Compositing
  first makes everything else measurable.

### Why the GPU path works (all enablers already in place)

1. **One shared device.** Ruffle renders on Bevy's wgpu device
   (Cargo.lock: exactly one wgpu, 30.0.2). No device boundary.
2. **The texture is already sampleable.** `TextureTarget::new`
   (`target.rs`) creates it `Rgba8Unorm` with usage
   `RENDER_ATTACHMENT | COPY_SRC | TEXTURE_BINDING` — `TEXTURE_BINDING`
   is already set. Same format as the Bevy `Image` in use today.
3. **A public handle exists — no patch needed.**
   `WgpuRenderBackend::target()` (backend.rs:387, `pub`) returns `&T`,
   and `TextureTarget::get_texture()` (target.rs:265, `pub`) returns a
   clone of the `wgpu::Texture`. The harness already downcasts to
   `WgpuRenderBackend<TextureTarget>` in `capture()` (runner.rs:496) —
   the same downcast reaches the texture.
4. **Queue order is the synchronization.** Ruffle submits its own
   command buffers directly via `queue.submit()` during `step()`
   (PrepareAssets, before the graph runs). Bevy's render graph submits
   after, on the same queue. Same queue = in-order execution, so a
   composite pass in the graph runs after Ruffle's render. wgpu handles
   the `RENDER_ATTACHMENT` → `TEXTURE_BINDING` usage transition across
   submissions.
5. **Precedent in-tree.** Ruffle's own `run_copy_pipeline`
   (utils.rs:199) samples a texture into a frame view with a bitmap
   bind group + fullscreen quad — the exact shader pattern the
   composite node needs.

### The fix shape (concrete)

1. **Harness accessor** (one new method, no Ruffle patch):
   `Harness::hud_texture() -> Option<wgpu::Texture>` — downcast like
   `capture()` does, then `backend.target().get_texture()`.
2. **Render-world resource**: hold the `wgpu::Texture` + a
   `TextureView` + sampler, created when the player builds (same place
   `Harness::new_shared` is called today). wgpu textures are refcounted
   — cloning the handle is cheap and the lifetime is safe (Ruffle owns
   the original; Bevy's clone keeps it alive).
3. **Optional one-line patch (0008)**: `TextureTarget::submit` encodes
   a `copy_texture_to_buffer` into the staging buffer whenever
   `target.buffer` is `Some`. `take_buffer(self)` takes by value and
   `target()` returns `&T`, so the harness cannot clear it without a
   patch. Add `WgpuRenderBackend<TextureTarget>::drop_staging_buffer()`
   (same pattern as patches 0001–0007). This saves one GPU copy per
   frame and frees the ~3.7 MB staging buffer. **Not required** —
   leaving the copy in place costs only that GPU copy; `capture_frame()`
   is simply never called.
4. **Custom render-graph node** (the M6 plan already named in
   ruffle.rs's module docs): a Bevy `RenderGraph` node with a
   fullscreen-triangle shader sampling the Ruffle texture, writing to
   the current view. Insert it **between the 3D main pass and the UI
   pass**, so the HUD draws over the 3D scene but under the modal Bevy
   menus (pause/inventory/dialogue) — matching the current `ImageNode`
   z-behaviour. Exact edge names to verify against bevy_pbr 0.20.1's
   graph setup at implementation time.
5. **Delete the CPU path**: remove `capture()` from the render system,
   the mailbox write, and `upload_ruffle_hud_frame`'s per-frame
   `image.data` replacement. Keep the Bevy HUD fallback: the composite
   node is gated on the same `enabled && has_frame` condition, and the
   Bevy HUD shows when Ruffle is off or failed (as today).
6. **Latency drops** from 1 frame (mailbox) to 0 extra frames
   (same-frame graph node). The native-resolution HUD work (Milestone
   2 — raising the render target above 1280×720) composes with this:
   a bigger target costs more GPU raster, no readback.

### Obstacles and risks

- **Resize**: the player is built once at a fixed size; a resize
  currently stretches the texture. Compositing doesn't change this —
  rebuild-on-resize is separate, already a known M4 limitation.
- **Present-mode backpressure**: if the GPU is slower than the CPU,
  the frame time becomes GPU time via present acquire. The saving is
  bounded by the GPU cost (see the estimate).
- **Fallback gating**: the node must hide when Ruffle is disabled or
  failed, or the HUD would freeze on a stale texture. Same condition
  as today's visibility system, moved into the render world.
- **No new failure modes in the Ruffle direction**: the player build,
  settle, drive, and step paths are untouched. Only the output path
  changes.

### Estimated saving (from the log numbers)

Per-frame Ruffle cost today is `run + render + capture`, plus the
unmeasured re-upload inside the non-Ruffle remainder. Compositing
removes capture + the re-upload. It does NOT remove `run` (AS2,
9–12 ms CPU), `render` CPU encode (~6 ms), or the Ruffle GPU
rasterization (still executes, now pipelined).

- **Inn**: ~98 ms → **~55–65 ms** (removes ~32 ms capture + a few ms
  upload; ~15–18 fps).
- **Late exterior**: ~62 ms → **~50–55 ms** (capture was already only
  ~4 ms; the saving is the upload + residual stall; ~18–20 fps).
- **Early in a scene**: the saving is largest (~30 ms) because the
  stall is largest.

Bottom line: compositing is worth **~30 ms/frame early, ~5–10 ms/frame
at steady state**. It does not by itself reach 30 fps — the ~40 ms
non-Ruffle cost (Part 2) is the remaining wall. Do compositing first
anyway: it is the biggest single lever, and it removes the stall that
makes every other cost unmeasurable.

## Part 2 — the ~40 ms non-Ruffle cost

### The decomposition

Late exterior (prev w35–37 vs ruffle w48–50): frame 62.5–63.0 ms,
Ruffle 22 ms → **non-Ruffle ≈ 40 ms**. Inn (curr w2–5 vs ruffle w0–3):
frame 97–99 ms, Ruffle ~48 ms → **non-Ruffle ≈ 49 ms**. The inn is
~9 ms worse despite fewer meshes (815 vs 2216 sub-meshes) — see
suspect 3.

### Ranked suspects

1. **The 3D main pass — draw calls + fragment cost (~20–30 ms,
   largest).** Exterior: 2216 sub-meshes, each its own entity with its
   own Mesh/Material — Bevy issues one draw per entity per pass, no
   batching, no instancing, no mesh merging. Inn: 815. Per-draw CPU
   overhead (bind groups, pipeline state) plus GPU fragment cost
   (full-screen overdraw: terrain + water + sky dome + alpha-tested
   foliage, all with normal maps on the phone — desktop exteriors use
   `--no-normal-maps`). Fits the evidence: frame time improves as
   pipelines/bind groups warm up, then plateaus at a draw-count-driven
   floor.
2. **The Bevy Image re-upload (~3–10 ms, unmeasured).** `image.data =
   Some(pixels)` every frame → `RenderAssets<Image>` re-uploads 3.7 MB
   via `queue.write_texture`. Inside the 40 ms; disappears with M6.
3. **Point-light shadow faces — inn only (~9 ms).** 2 shadow lights ×
   6 cubemap faces @ 512² = 12 extra renders of the shadow-casting
   subset. Matches the inn/exterior ~9 ms delta. The exterior has NO
   shadows at all (sun `shadow_maps_enabled: false`, 0 point lights),
   so shadows cannot explain the exterior's 40 ms.
4. **Present/vsync backpressure (unknown share).** If the GPU takes
   ~40 ms to execute the frame, the CPU blocks at swapchain acquire
   (FIFO present) and the wait lands in the measured delta. Current
   instrumentation cannot distinguish CPU-bound from GPU-bound — this
   is the open question the instrumentation below must answer.
5. **Extract + render-world CPU overhead (~2–5 ms).** Every frame:
   extract copies main→render world (1000+ entities), `prepare_clusters`
   rebuilds the light-cluster bindings (confirmed running every frame
   by the cluster-diag line), bind groups, uniform uploads.
6. **Bevy UI layout + text (~1–3 ms).** `update_touch_ui` writes the
   joystick-knob `Node` and every button's `ImageNode` every frame
   (Bevy change detection fires on the `&mut` access even when values
   are unchanged → UI systems re-run); the compass heading text changes
   every frame (glyph re-layout). Small subtree, small cost.
7. **Animation / player / collision systems (<1 ms).** Mill wheel,
   sawmill, water flow, player movement, combat/archery queries — a
   handful of transform writes and small queries. Not the 40 ms.
8. **`log_cluster_buffer_sizes` (negligible).** Only formats a string
   when counts change, throttled to 1 line per 60 frames.

### What instrumentation would prove it

Current telemetry measures only whole-frame CPU delta + Ruffle step
costs. It cannot split the 40 ms. Add, in priority order:

1. **Phase timing in the render world.** Timestamp the frame's phases
   and log per 5 s window (same pattern as `perf.rs`): (a) main-world
   Update time, (b) extract time, (c) render-schedule time (systems +
   graph execution), (d) the remainder of the delta = present wait.
   Implementation: a system at the start of Update and one in
   `RenderSystems::PrepareAssets` / `Cleanup` bracketing the render
   world. This alone splits CPU (Update vs render) from present-wait.
2. **GPU timestamps.** wgpu `TimestampQuery` around the main pass and
   the shadow passes (or Bevy's `bevy::render::diagnostic` if it is
   wired). The only way to separate GPU execution time from CPU
   submission time. If GPU main-pass ≈ 35 ms, the frame is GPU-bound
   (fix: fewer draws / cheaper fragments). If GPU ≈ 10 ms, it is
   CPU-bound (fix: draw-call batching / fewer entities).
3. **Draw-call + triangle counts per frame.** One line per window:
   draws, triangles, shadow views (from the render phases / visible
   entities). Correlate with frame time.
4. **The cheapest decisive experiment — Ruffle HUD off.** Run the
   phone with the Ruffle HUD disabled (the Bevy HUD fallback already
   exists; no code change, just the setting) and compare `perf:`
   windows. This isolates the ENTIRE Ruffle + upload cost in one
   number, and validates the compositing estimate before the work is
   done.
5. **Look-at-the-sky test.** Frame time facing the sky (few draws, no
   shadows) vs facing the town. A ~30 ms delta means draw/fragment-
   bound; ~0 means fixed per-frame overhead (extract/UI/present).

### Two cheap wins noticed along the way

- **Gate the Ruffle render system to InGame.** It currently has no
  state gate, so the movie keeps stepping during door transitions
  (Spawning). Prev log window 52 shows `render avg 105.0 ms, max
  396.6 ms` across the +365 s scene transition — the movie's render
  cost explodes exactly when the HUD isn't visible. One `run_if`
  (or an early return on state) removes it.
- **`run` grows over a run** (9.1 ms → 12.6 ms across the prev log).
  Worth one look: whether the movie accumulates work (more active
  clips / growing display list) or it is thermal throttling on the
  phone. Do not chase it before the phase timing exists.

## What to do first (ordered)

1. **The Ruffle-off A/B** (experiment 4, no code) — validates the
   compositing estimate in one phone run.
2. **GPU compositing** (Part 1) — the biggest lever, and it unblocks
   clean measurement of everything else.
3. **Phase timing + GPU timestamps** (instrumentation 1–2) — split the
   remaining ~40 ms into CPU vs GPU before choosing the next fix.
4. **The InGame gate** (cheap win) — fold into the compositing work.
5. **Draw-call reduction** (only after step 3 says which side is
   binding): batching/merging for the 2216 exterior draws if
   CPU-bound; cheaper fragments / fewer overdraw layers if GPU-bound.
