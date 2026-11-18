# v0.4.8 phone crash + missing Ruffle HUD — diagnosis and fix

Date: 2026-11-16. Build: v0.4.8 (delivered 11:42 EST). Phone log:
`gameplay/dovahengine-startup-v048-20261116.log`. Fix: v0.4.9, in
`crates/dovah-app/src/ruffle.rs` only. No `lib.rs` change needed.

## The crash

At +110.555 s (~11 s after the inn finished loading, no interaction
in between), the app panicked:

```
PANIC: Trying to destroy a SwapchainAcquireSemaphore that is still in use
by a SurfaceTexture at wgpu-hal-30.0.2/src/vulkan/swapchain/native.rs:400:58
```

The backtrace is the RENDER world's tables being cleared (`BlobArray::clear
→ Column::drop → Table::drop → Tables drop`), i.e. full `App` teardown —
not a surface reconfigure. `App::run()` never returned (no "clean exit"
line). v0.4.6 exited clean after 6+ min, so this is new in v0.4.8 (or new
to this session).

## Root cause (proven from source)

1. **A `SurfaceTexture` holds a clone of the swapchain's acquire
   semaphore** (`wgpu-hal .../vulkan/swapchain/native.rs`, in
   `acquire()`). `release_resources()` destroys each semaphore with
   `Arc::into_inner(...).expect(...)` — it PANICS if a `SurfaceTexture`
   is still alive when the `Surface` drops. `release_resources` already
   called `device_wait_idle`, so this is purely a CPU-side lifetime
   issue: a `SurfaceTexture` object was still alive at teardown.
2. **The only live `SurfaceTexture` in Bevy is
   `ExtractedWindow.swap_chain_texture`** (bevy_render 0.20.1
   `view/window/mod.rs`). It is acquired in `prepare_windows` and
   normally presented (taken) by `render_system` — but present runs
   only if `view_needs_present || needs_initial_present`, and
   `view_needs_present` is false when no view rendered to the window
   that frame (`OutputColorAttachment::needs_present` =
   `get_attachment` was called at least once). A skipped present
   leaves the texture alive into teardown.
3. **No ECS removal can fix the drop order.** Table columns drop in
   ascending `ComponentId` order (bevy_ecs asserts the columns are
   sorted in `TableBuilder::build`; both `Table::drop` and
   `move_row` iterate in that order) — and in the render world
   `SurfaceData`'s id is lower than `ExtractedWindow`'s, so the
   surface always drops FIRST, whether via Bevy's `WindowClosing`
   removal (`despawn_windows` → `extract_windows`) or the render
   world's drop. The v0.4.8 backtrace shows the world drop.
   Conclusion: the texture must be dropped EXPLICITLY, after the
   last present, before the `App` is dropped.
4. **The exit was `AppExit`-triggered.** On Android, winit 0.30.13
   never exits the loop on activity destroy (it only logs
   `TODO: forward onDestroy notification`); the ONLY
   `event_loop.exit()` in the stack is bevy_winit `state.rs:763`,
   gated on `App::should_exit()`, which is `Some` only when
   `Messages<AppExit>` is non-empty. Reachable writers on the phone:
   pause Quit (tap or E-confirm) and bevy_window's
   `exit_on_all_closed` (default `ExitCondition::OnAllClosed` in the
   windowed path). Which one fired is UNPROVEN from the log (no lines
   between +99.6 s and +110.6 s) — the fix covers all of them.

## Exonerated (with evidence)

- **Ruffle is NOT involved.** The Ruffle player renders to its own
  offscreen `TextureTarget` and never touches the swapchain. It runs
  in the render world on the same thread (not a separate thread);
  its failures are non-fatal `eprintln!`; a Ruffle panic would be
  its own `PANIC` line — the only one is the surface panic.
- **Back button.** winit Android maps it to `BrowserBack`; nothing
  listens for it; winit never sends `CloseRequested` on Android.
- **Activity destroy / OS kill.** winit does not exit the loop on
  destroy; an OS kill produces no graceful teardown and no panic log.
- **Screenshot exit.** `ScreenshotPlugin` is only added in the
  headless path, not the phone binary.

## The fix (crash) — `crates/dovah-app/src/ruffle.rs`

Two systems + one resource, registered in `RuffleHudPlugin` (the only
existing plugin with render-app access; the fix is NOT
Ruffle-specific):

- `ExitRequested(bool)` — render-world resource, latched.
- `extract_app_exit` (`ExtractSchedule`) — reads
  `Extract<MessageReader<AppExit>>`; sets the flag when an `AppExit`
  exists. Extract always runs in the same update that wrote the
  `AppExit` (AppExit is only written by a system, and every update
  runs main schedule → extract → render schedule).
- `drop_swapchain_texture_on_exit` (`RenderSystems::Cleanup`) — if
  the flag is set, `.take()`s (drops) `swap_chain_texture` from every
  `ExtractedWindow`. `Cleanup` runs after `Render` (where present
  happens) in the same update — the last chance before the loop
  exits and `App::run()` drops the render world. Dropping a
  `SurfaceTexture` without presenting just releases it back to the
  swapchain; `ExtractedWindow::present` handles `None` gracefully.

Self-healing: if the app somehow did not exit, the next frame
re-acquires a texture (`prepare_windows` acquires when none is
present), and the post-present `take()` is a no-op.

Alternatives considered and rejected: vendoring wgpu-hal to soften
the `expect` (robust but a huge rebuild + a forked core dep);
`std::process::exit(0)` on Quit (a sidestep, and it would not cover
`exit_on_all_closed`); a custom winit runner (fragile, depends on
bevy_winit internals). The chosen fix is trigger-agnostic and stays
in our code.

## The missing Ruffle HUD

Dylan saw the Bevy HUD, never the Ruffle HUD. The phone log has
ZERO Ruffle lines — but that proves nothing by itself:

- **All Ruffle logging was `eprintln!` (stderr), which is invisible
  on Android.** The phone log captures ONLY `startup_log::` lines +
  the panic hook. The player could have built and failed silently, or
  never built — the log cannot distinguish them.
- **Settings exonerated.** `GameSettings.ruffle_hud` has
  `#[serde(default = "default_ruffle_hud")]` → `true`, and
  `load_settings` falls back to `Default` (also `true`) on any
  failure — old settings files without the field resolve to `true`.
- **Player-build failure modes exonerated one by one.**
  `SwfMovie::from_data` (same bytes as desktop), `TextureTarget::new`
  (size ≤ 1280×720 vs Adreno max 16384), `WgpuRenderBackend::new`
  (only fails on size or `GpuProfiler::new`, which does no GPU work
  — just `settings.validate()` + channels). The build almost
  certainly SUCCEEDS on the phone; the failure is downstream
  (settle / `InitExtensions` / drive / capture) or the `enabled`
  gate — unproven without phone-side logs.

## The fix (HUD) — instrumentation in `ruffle.rs`

Every Ruffle stage now logs once to `startup_log` (the only channel
that reaches the phone log file), alongside the existing `eprintln!`:

- `prepare_ruffle_hud_assets`: success (byte counts + size) and each
  failure (missing `hudmenu.gfx` / missing `fonts_en.swf` / fontlib
  error).
- `drive_ruffle_hud_state`: the resolved `enabled` flag + its source
  (settings vs CLI override), once.
- `ruffle_hud_render_system`: player-build success / failure (with
  the error), and the first captured frame (size), once.
- `upload_ruffle_hud_frame`: the first upload to the Bevy image
  (size), once — proves the whole pipeline.

The next phone run's log will pinpoint the failing stage by which
line is the LAST one present.

## Verification

- `cargo test -p dovah-app --lib`: **235 passed, 0 failed, 1 ignored**
  (was 230 at the v0.4.7 round; +5 are Ultron's in-flight collision
  tests). Includes `windowed_app_builds_and_runs_two_updates`, which
  builds the app with the new systems registered.
- `rustfmt --check` on `ruffle.rs`: clean.
- `cargo clippy -p dovah-app --lib`: **0 warnings in `ruffle.rs`**.
  (The workspace is NOT clippy-clean right now: 3 warnings in
  `scene.rs` + 4 in `dovah-nif`, all in Ultron's in-flight collision
  code — not this fix's scope.)
- Desktop screenshot run: SKIPPED (deliberate). The box was busy
  with Ultron's collision iteration (back-to-back test runs) and
  the marginal value was low: the lib test proves the plugin builds
  with the new systems; the `Extract<MessageReader<AppExit>>`
  pattern is identical to bevy_render's own `extract_windows`; the
  `Cleanup` system is a bool check + `Option::take` (no panicking
  operations); and the M4 desktop proofs already established that
  the Ruffle pipeline reaches every logged stage on desktop. The
  desktop screenshot path is headless (no window, no surface), so
  it cannot reproduce the Android teardown anyway — the phone is
  the real proof (see below).

## What remains unproven (phone proof needed)

1. **The crash fix cannot be proven on desktop.** The desktop
   screenshot path is headless (no window, no surface, no
   `SurfaceTexture`). The proof is on the phone: open pause → Quit
   (and: play past 110 s, then Quit) — the app must close WITHOUT
   the semaphore panic, and the next run's log must show no panic.
2. **Which `AppExit` sender fired in v0.4.8** (pause Quit vs
   `exit_on_all_closed`) — unknowable from the v0.4.8 log; the fix
   covers both, so it does not matter for the fix.
3. **Why present was skipped in the exit update** (which left the
   texture alive) — the frame-level trigger is unproven; the fix
   drops the texture regardless of why it was not presented.
4. **Which HUD stage fails on the phone** — the instrumentation
   answers this on the next run; if the log shows "first frame
   uploaded" and Dylan STILL sees the Bevy HUD, the fault is in
   `update_ruffle_hud_visibility` / the node, not the pipeline.
5. **Residual risk:** if the render device were lost, the Render
   schedule would not run and the `Cleanup` system would not drop
   the texture — but device loss has no evidence in the v0.4.8 log
   (the scene rendered fine until the panic).
