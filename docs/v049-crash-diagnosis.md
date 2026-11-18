# v0.4.9 phone crash + missing Ruffle HUD — diagnosis, fix, and instrumentation

Date: 2026-11-16. Build: v0.4.9 (delivered ~16:20 EST). Phone log:
`gameplay/dovahengine-startup-v049-20261116.log`. Fix + instrumentation:
in `crates/dovah-app/src/ruffle.rs`, `crates/ruffle-harness/src/runner.rs`,
and one accessor in `vendor/bevy_pbr/src/cluster/mod.rs`. No APK build —
the next build carries it.

Dylan's report: "Same old ui you mad crashed again" — Bevy HUD, not the
real HUD, and it crashed.

## The log (facts)

- Load clean: ESM 920,181 records (~21 s); 18 BSAs / 177,748 files; inn
  cell 437 objects; scene build 419 real-mesh objects (815 sub-meshes),
  0 placeholders, 0 missing textures, 3 actors, 8 lights, 5 doors.
- **Collision is LIVE on the phone**: "ObstacleGrid built: 8710 cells
  with data, 10588 intervals" — matches the desktop inn numbers exactly.
- Ruffle stages (the v0.4.9 instrumentation worked):
  - +92.694 s: "prepared hudmenu.gfx (181594 bytes) + fontlib (610201
    bytes) at 797x384" — prepare OK.
  - +95.554 s: "enabled=true (from settings)" — gate ON.
  - +96.717 s: "player built on the shared device (797x384)" — player
    built OK.
  - NO "first frame captured" and NO "first frame uploaded" by the
    panic at +105.106 s — 8.4 s with a built player and no capture.
- NEW PANIC at +105.106 s (~10 s into InGame frames), NOT the v0.4.8
  swapchain one:

```
PANIC: Failed to get mapped range for buffer created with mapped_at_creation:
MapRangeError("Validation Error — Buffer with '' label is invalid")
at wgpu-30.0.2/src/util/device.rs:72
```

  Backtrace (truncated at frame 19 by the panic hook's cap):
  `DeviceExt::create_buffer_init` ← `RenderDevice::create_buffer_with_data`
  ← `StorageBuffer<GpuClusterableObjectIndexListsStorage>::write_buffer`
  ← `ViewClusterBindings::write_buffers` ←
  `prepare_clusters_for_cpu_clustering` (frame 15 — CPU clustering).
- +106.517 s: "main app: app.run() returned (clean exit)" — 1.411 s
  after the panic. NO semaphore panic anywhere in the log.

## Finding 1 — the HUD never reached its first capture

### Proven from code

1. **The capture call was never reached.** `capture()` can NEVER return
   `None` for this target: `TextureTarget::new` creates the staging
   buffer eagerly (`buffer: Some(...)`, vendored Ruffle
   `render/wgpu/src/target.rs`); `capture_frame()` (backend.rs:202)
   returns `Some(buffer_to_image(...))` whenever the buffer exists;
   `buffer_to_image`/`capture_image` (utils.rs) always returns an image
   or PANICS (`device.poll(Wait)` + expects). The downcast to
   `WgpuRenderBackend<TextureTarget>` always succeeds (constructed that
   way). The code NEVER REACHED the capture call in 8.4 s — the failure
   is strictly upstream (enabled / settle / time accumulator).
2. **Enabled gate exonerated.** `extract_ruffle_hud` copies `HudState`
   to the render world every frame unconditionally; the player built
   AFTER the enabled=true log, so the render world had enabled=true.
3. **Size/alignment exonerated.** 797x384 target; `BufferDimensions`
   pads bytes_per_row to a multiple of 256 (797×4=3188 → padded 3328);
   the copy in `TextureTarget::submit` uses the padded value.
4. **Slow-first-render-via-pipeline-compile weakened.** Ruffle's
   pipelines are created at player-build time (`Surface::new` →
   `Descriptors::pipelines` → `Pipelines::new`), not lazily at first
   render.
5. **Remaining explanations:** (a) fewer than 11 render frames ran in
   8.4 s (the app at ~1 fps — each frame dominated by something slow,
   most plausibly `harness.step()` itself, the only new per-frame
   render-thread work, running in `PrepareAssets` before everything
   else); or (b) `delta_secs` was 0 every frame (implausible — Bevy
   Time works on Android).
6. **Evidence FOR ~1 fps:** the 1.4 s panic-to-exit gap — the
   RenderError exit path (see Finding 3) fires at the NEXT extract
   after the error is stored, ~16 ms later at 60 fps but ~1 frame
   later at ~1 fps. The HUD stall (10 settle frames needing >8.4 s)
   and the exit gap both fit ~1 fps.
7. **Evidence AGAINST slow steps:** v0.4.8 ran ~11 s in-game with the
   IDENTICAL Ruffle render code (only instrumentation differs in
   v0.4.9) and Dylan reported no freeze. Weak evidence — he may not
   have interacted, and 11 s is short — but it counts against the
   slow-step hypothesis.

### What changed in v0.4.9 vs v0.4.8 (cluster-relevant)

There is no git repo, so the diff is reconstructed from the delivery
record. The v0.4.9 changes were: (1) the swapchain teardown fix
(runs only on exit), (2) Ruffle HUD instrumentation (logging only),
(3) per-object collision (CPU-only `ObstacleGrid` + `move_blocked`;
no GPU resources, no lights/views/cameras). None of them touch
clustering. The vendored `bevy_pbr` is unchanged since the linear
light-falloff patch (pre-v0.4.8). The cluster panic is therefore NOT
a v0.4.9 regression — it is a device-state event (see Finding 2)
that the v0.4.8 run did not live long enough to hit (v0.4.8 exited
at ~11 s via the teardown path; v0.4.9's cluster panic hit at ~10 s
into InGame, before any quit).

### The fix + instrumentation (HUD)

In `crates/ruffle-harness/src/runner.rs`:

- **`Harness::step_logic()`** — executor pump + `preload` +
  `run_frame`, NO `render`. The settle phase now uses it: settle
  frames are never captured, so rasterizing them is wasted GPU work.
  The fontlib import completes via the executor pump + preload, and
  frame-1 scripts run in `run_frame` — neither needs a render. The
  first post-settle `step()` renders normally. If the render is the
  slow part, this makes the settle ~free; if the logic is the slow
  part, the timing logs (below) will show it.

In `crates/dovah-app/src/ruffle.rs` (`ruffle_hud_render_system`):

- **Force the first post-settle step + capture regardless of the
  clock.** If the clock gate is the blocker, the HUD must still
  appear, and the first-frame log must still fire. This both fixes a
  potential clock-gate issue AND sharpens the diagnostic: if the
  forced step still produces no capture, the failure is in
  `step()`/`capture()` itself, not the clock.
- **Instrumentation** (all to `startup_log` — `eprintln` is invisible
  on Android — one-time or throttled):
  - "settle step 1 (preload+run=Xms)" — the phone's per-step cost.
  - "slow settle step" / "slow movie step" (>100 ms, one-time) —
    with the preload+run vs render split, so the next log says
    WHICH half is slow.
  - "settled (10 frames), InitExtensions called" — settle completion.
  - "first movie step (delta=Xs, accum=Xs, preload+run=Xms,
    render=Xms)" — the clock values + cost of the first post-settle
    step.
  - "capture returned None after step" (one-time) — was silent; it
    should never happen for this target (see §1 above), so a `None`
    would mean the renderer's downcast failed.
  - "teardown fix: dropped live swapchain texture on exit" (in
    `drop_swapchain_texture_on_exit`) — proves the v0.4.8 fix was
    actually exercised on the next run.

### What the next phone log will distinguish

- "settle step 1" showing ~800 ms: slow steps confirmed. If the HUD
  then appears (settle is now render-free), the fix worked.
- Settle completes but "first movie step" never appears: the
  post-settle branch is not reached — the forced step rules this out
  if it works.
- "first movie step" appears but no capture: the failure is in
  `step()`/`capture()` itself.
- "capture returned None": the downcast failed (should not happen).

## Finding 2 — the cluster-buffer panic (device loss or OOM, NOT size)

### Proven from code

1. **`create_buffer_init` pads size to a multiple of 4, minimum 4**
   (wgpu 30.0.2 `util/device.rs`) — zero-size and misalignment are
   IMPOSSIBLE.
2. **wgpu-core `create_buffer` validation** (wgpu-core-30.0.2
   `device/resource.rs:1005-1090`) failure modes: `check_is_valid`
   (DeviceError::Lost), MaxBufferSize, InvalidUsage, UsageMismatch,
   UnalignedSize. For our descriptor (padded size, usage
   COPY_DST|STORAGE, mapped_at_creation=true): InvalidUsage,
   UsageMismatch, UnalignedSize are impossible.
3. **MaxBufferSize is impossible.** Cluster dims are capped (default
   FixedZ total 4096, z_slices 24; `bevy_light` `assign.rs` bounds
   the product at 4096), so max indices = 8 lights × 4096 clusters
   = 32,768 × 4 B = 128 KB, vs an Adreno `max_buffer_size` of
   ≥256 MB.
4. **Remaining: device lost OR HAL allocation failure (OOM)** via
   `handle_hal_error_with_nonfatal_oom`. Note: `mapped_at_creation`
   WITHOUT MAP_WRITE takes wgpu-core's staging-buffer branch —
   creation allocates a SECOND staging buffer of the same size.
5. **`prepare_clusters_for_cpu_clustering`** (vendored `bevy_pbr`
   `cluster/mod.rs:478`) creates a FRESH `ViewClusterBindings`
   (fresh `StorageBuffer::default`, buffer=None) EVERY frame per
   view, so `write_buffer` calls `create_buffer_with_data` EVERY
   frame. ~600 creations succeeded before the one at +105 s failed
   — the device state changed at +105 s, not the descriptor.
6. **GPU clustering is DISABLED on Android** (`make_global_cluster_settings`:
   `cfg!(target_os = "android")` → gpu_clustering None) — the phone
   uses CPU clustering, consistent with the backtrace.
7. **The actual wgpu error was captured by Bevy but never reached
   the phone log.** Bevy's `DeviceErrorHandler` (bevy_render
   `error_handler.rs:126-150`) installs `set_device_lost_callback` +
   `on_uncaptured_error`, both logging via `bevy_log::error!`
   (tracing — INVISIBLE in the phone log file). `poll()` returns a
   `RenderError`; the DEFAULT `RenderErrorHandler`
   (error_handler.rs:69-83) writes `AppExit::error()` and stops
   rendering — the likely exit mechanism behind the clean exit at
   +106.517 s. `create_buffer_with_data` is a direct pass-through
   to `device.create_buffer_init` (NO error scope), so the
   validation error goes to the uncaptured-error handler and is
   queued as a `RenderError`.

### The diagnostic (no code fix is justified yet)

No vendored patch is justified: the size is bounded by construction
(nothing to guard); the `expect` is inside wgpu's
`create_buffer_init` (not patchable without forking wgpu); and a
failed cluster buffer means the view cannot render — the app should
exit, which Bevy's `RenderErrorHandler` already does. The
deliverable is a diagnostic that makes the next phone log answer
which failure it was.

In `crates/dovah-app/src/ruffle.rs` (registered in `RuffleHudPlugin`,
the only plugin with render-app access):

- **`install_wgpu_error_handlers`** (`RenderStartup`, once):
  replaces Bevy's uncaptured-error handler + device-lost callback
  with handlers that log to `startup_log` ("wgpu error: ..." /
  "wgpu device lost: Unknown/Destroyed — ...") AND set a static
  flag. Both wgpu setters REPLACE (single slot — verified in
  `wgpu-30.0.2/src/backend/wgpu_core.rs`). Ruffle installs no error
  handler of its own (checked 2026-11-16), so nothing replaces
  these afterwards. Runs in `RenderStartup` — after `RenderPlugin`
  installed Bevy's handlers, so these replace them.
- **`exit_on_device_error`** (main-world `Update`, every frame, no
  state gate): when the flag is set, writes `AppExit::error()` —
  preserving Bevy's exit-on-render-error behaviour EXACTLY (Bevy's
  default handler quits for any `RenderError`). Without the flag,
  replacing Bevy's handlers would turn a device loss into a hang
  (the render would keep failing on a lost device) instead of a
  clean exit.
- **`log_cluster_buffer_sizes`** (`RenderSystems::Queue` — after
  `Prepare`, so the counts are the current frame's): logs the
  adapter + `max_buffer_size` once, then the live `n_indices` +
  `n_offsets` across all views (with the implied byte sizes:
  indices × 4 B, offsets × 32 B) whenever they change, throttled to
  one line per 60 frames, with the FIRST line immediate (at ~1 fps
  the 60-frame throttle would delay it past the v0.4.9 panic
  point). The last line before a panic is the size that was being
  requested.

In `vendor/bevy_pbr/src/cluster/mod.rs`:

- **`ViewClusterBindings::n_offsets()`** (3 lines, mirrors the
  existing `n_indices()`) — the offsets/counts buffer is created
  the same way and could fail the same way, so the diagnostic logs
  both sizes.

### What the next phone log will distinguish

- "wgpu error: ..." with the exact validation text → the precise
  failure (OOM vs validation detail).
- "wgpu device lost: Unknown/Destroyed — ..." → device loss, with
  the reason.
- Small cluster sizes + a panic → size exonerated (as predicted);
  device loss / OOM confirmed as the cause.
- Sizes EXPLODING before the panic → the size hypothesis revives
  (would contradict the bound — indicating a cluster-count bug).

## Finding 3 — panic containment + the clean exit

### Proven from code

- **NO `catch_unwind`** in: dovah-app main (`lib.rs:792-793`,
  `app.run()` then the "clean exit" log), the winit Android event
  loop, `bevy_app` `App::run`/`App::update`, bevy_winit
  `winit_runner`, bevy_android, `#[bevy_main]` (bevy_derive
  `bevy_main.rs` — just generates `android_main` calling `main()`).
- **Bevy 0.20's multi-threaded executor DOES catch system panics:**
  `handle_errors` (bevy_ecs `multi_threaded.rs:846-882`) wraps every
  system in `catch_unwind`, calls the error handler with
  `BevyError::panic` (Severity::Panic); the default handler
  `match_severity` → `panic()` → `resume_unwind(payload)`; the
  executor stores the payload and resumes unwinding at schedule end
  (`multi_threaded.rs:306-308`). The Render schedule uses the
  default (multi-threaded) executor; only RenderStartup uses
  `SingleThreadedExecutor` (bevy_render `lib.rs:460`). No Bevy code
  inserts a non-default `FallbackErrorHandler` into the render
  world. `run_render_schedule` uses `try_run_schedule` (no panic
  catch — "try" is about a missing schedule label).
- **android-activity 0.6.1 `rust_glue_entry`** (`native_activity/glue.rs`
  ~976-987) wraps the whole `android_main(app)` in `catch_unwind`
  ("specifically catch any panic from the application's android_main
  so we can finish + destroy the Activity gracefully via the JVM"),
  logs via `log_panic`, then calls `ANativeActivity_finish`. This is
  the OUTER containment.
- **The exit mechanism:** the failed buffer creation ALSO delivers a
  validation error to Bevy's uncaptured-error handler (see Finding
  2 §7) → queued as a `RenderError` → the next extract's `poll()`
  returns it → the default `RenderErrorHandler` writes
  `AppExit::error()` → the app exits cleanly at the end of that
  update. The 1.4 s panic-to-exit gap fits this path only if the app
  was at ~1 fps (consistent with Finding 1).

### The open contradiction — TESTED, still unresolved

The code says the panic should propagate out of `main()` (the
executor resumes it) — but the log shows `app.run()` RETURNING.
The empirical test `system_panic_propagates_out_of_app_update`
(`ruffle.rs` tests) PROVES propagation on desktop: a panicking
system in `Update` (MinimalPlugins) escapes `App::update()` via
`catch_unwind` — so the executor does NOT swallow panics. Every
layer of the render path was also verified to propagate by reading:
the multi-threaded executor resumes at schedule end
(`multi_threaded.rs:306-308`); `try_run_schedule` has no catch
("try" is only about a missing schedule label);
`run_render_schedule` / RenderRecovery propagate; `SubApps::update`
/ `App::update` propagate; winit's Android event loop
(`poll_events`) propagates; android-activity's `poll_events`
propagates. android-activity's catches all RESUME or ABORT, none
swallow on the frame path: the input-event catch
(`native_activity/mod.rs:590`) calls `finish_event` (ANR guard)
then `resume_unwind`; `abort_on_panic` (`util.rs:59`) aborts the
process; the UI-thread-callback catch (`main_callbacks.rs:203`)
swallows, but only for callbacks posted to the Java UI thread, not
the frame update; `rust_glue_entry`'s catch (`glue.rs:976`) is
OUTSIDE `main()`, so the clean-exit line (which sits after
`app.run()` in the same function) could not print through it.

So the containment mechanism on the phone is UNRESOLVED: the code
and the desktop test say the panic must propagate out of `main()`,
but the log proves `app.run()` returned. The two remaining
possibilities are (a) the render sub-app path contains the panic
by a mechanism the main-schedule test does not exercise (a
render-schedule test is not possible in the test harness —
`WinitPlugin` refuses to build off the main thread, where test
threads run), or (b) something Android/aarch64-specific in the
release build. The exit itself is explained IF containment
occurred: the failed buffer creation ALSO queued a validation
error in Bevy's `DeviceErrorHandler`, so the next extract's
`update_state` (which runs during extract, before the render
schedule) found it, the default `RenderErrorHandler` wrote
`AppExit::error()`, and the app exited cleanly at the end of that
update — the 1.4 s gap fits this path only if the app was at
~1 fps (consistent with Finding 1).

**Teardown-fix exercise status: UNPROVEN for v0.4.9.** The exit was
`AppExit`-triggered, so the exit update ran and the `Cleanup` system
ran — but whether a live `SurfaceTexture` actually existed at this
exit (i.e. whether the fix did anything this run) is unknown;
present may have succeeded on the final frames. The clean exit is
weak evidence FOR the fix, not proof. The new "dropped live
swapchain texture" log (Finding 1) will prove exercise on the next
run.

## Verification

- `cargo test -p dovah-app --lib`: **242 passed, 0 failed, 1 ignored**
  (baseline 236 / 1 ignored; +6 = this fix's
  `system_panic_propagates_out_of_app_update` + Ultron's 5 in-flight
  water tests). Includes `windowed_app_builds_and_runs_two_updates`,
  which builds the app with the new systems registered, and the
  panic-propagation test (passed — the panic DOES propagate out of
  `App::update`, see Finding 3). Run took ~23 min (bevy_pbr recompile
  + dovah-app + test-binary link).
- `cargo test -p ruffle-harness`: **18 passed, 0 failed**.
- `cargo clippy -p dovah-app --lib`: **0 warnings** (verified on a
  forced re-check of the edited `ruffle.rs` — the clippy cache probe
  confirmed the recompile happened; the probe comment was removed
  afterwards).
- `cargo clippy -p ruffle-harness`: **0 warnings** (forced re-check
  of the edited `runner.rs`).
- `cargo clippy --all-targets` FAILS on Ultron's in-flight example
  `visual_tris` (`cannot find value 'cx' in this scope`) — NOT this
  fix's code; left for Ultron.
- `rustfmt --check` on the three changed files: the only diffs are
  3 PRE-EXISTING import-ordering lines in `ruffle.rs` (untouched by
  this fix); all new code is fmt-clean. The pre-existing diffs were
  left alone (out of scope, and reformatting would churn Ultron's
  working files).
- Desktop screenshot run with `--ruffle-hud`: SKIPPED (deliberate).
  Ultron's water renders occupied the box (back-to-back exterior
  lavapipe renders — a concurrent build/render OOMs the 6 GB box,
  proven 2026-11-16), and the marginal value was low: the lib test
  proves the plugin builds with the new systems, `step_logic`
  shares `step()`'s code path (executor pump + preload + run_frame),
  and the desktop screenshot path cannot reproduce a phone GPU
  validation failure anyway. The next phone run is the real proof.
- Harness timing run (`ruffle-harness` binary, per-step timings):
  SKIPPED for the same reason (box busy with Ultron's renders).
  Desktop lavapipe timings would not prove phone timings anyway
  (software rasterizer vs Adreno — the render split is not
  comparable); the new phone instrumentation (settle-step timing
  lines) answers the per-step-cost question directly on the next
  phone run.

Desktop screenshot runs cannot reproduce a phone GPU validation
failure (different GPU, different driver, no Adreno) — the cluster
diagnosis rests on code-level proof (the wgpu validation rules +
the buffer-size math), not on a desktop run.

## What remains unproven (phone proof needed)

1. Whether the HUD appears on the phone (settle + first capture +
   first upload lines).
2. Whether the per-step cost is ~800 ms on the phone (the
   settle-step timing lines).
3. Whether the cluster panic recurs, and if so, what the wgpu error
   text / device-lost reason says.
4. Whether the teardown fix actually drops a live texture (the new
   log line).
5. Whether the app runs past 110 s / quits cleanly.
6. The panic-propagation question: the desktop test PROVES the
   panic propagates out of `App::update`, so the phone's clean exit
   after a render-system panic has NO explained mechanism yet (see
   Finding 3). The next phone log's wgpu-error forwarding will show
   whether the RenderError path fired (which requires containment),
   narrowing it further.
