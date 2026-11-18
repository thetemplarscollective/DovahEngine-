# Dovahengine

A clean-room, open-source engine for *The Elder Scrolls V: Skyrim Special
Edition*, built from scratch by Dylan and Jarvis. Not a fork of Mudcrab or
OpenMW, and not affiliated with or endorsed by Bethesda Softworks, ZeniMax
Media, or Microsoft.

The long-term goal is a native engine that runs Skyrim on Dylan's phone as
an Android APK. The short-term goal is honest, verified pieces: each
milestone is a working, tested part of the engine before the next one
starts.

## Current state

**v0.4.13** (2026-11-17) — see `README-SOURCE.md` for the current layout,
the v0.4.13 phone playtest verdict, and what's under fix. The milestone
list below is the early history (M1–M8.1.3). The current work is the
lighting calibration and UI fixes for v0.4.14, the original Skyrim menus
via Ruffle, loading performance, and the original main menu + loading
screens. The diagnosis docs in `docs/` (`ruffle-gpu-compositing-`,
`loading-performance-`, `melee-no-hit-`, `m5-original-menus-`,
`milestone4-presentation-diagnosis.md`) are the current roadmap detail.

## Legal line

Dovahengine ships **no Bethesda code and no game assets**. It reads the
data files from a copy of Skyrim Special Edition the user already legally
owns. Never commit game data, extracted assets, or Bethesda binaries to
this repo.

## Layout

- `crates/dovah-bsa` — BSA archive reader (list + extract, v104/v105).
  Milestone 1, done and tested.
- `crates/dovah-esm` — ESM/ESP plugin parser (TES4 header, records,
  nested GRUPs, compressed records, XXXX oversized fields).
  Milestone 3, done and tested.
- `crates/dovah-nif` — NIF mesh loader (NiTriShape / NiTriStrips /
  BSTriShape, shader + texture resolution, plain mesh assembly).
  Milestone 4, done and tested.
- `crates/dovah-dds` — DDS texture reader (header + DX10 header,
  uncompressed RGB(A), BC1–BC7 decode to RGBA8 via `bcn_decode`).
  Milestone 4, done and tested.
- `crates/dovah-pex` — Papyrus script parser: compiled .pex bytecode
  (header/string table/objects/states/functions, full opcode table)
  + .psc structural parser. Milestone 5, done and tested.
- `crates/dovah-app` — Bevy 0.20 renderer: loads a CELL from the real
  Skyrim.esm, draws it (real NIF/DDS meshes when present, OBND
  placeholder boxes otherwise), first-person WASD/mouse, headless
  offscreen screenshot mode. Milestone 6, done and tested.
- `screenshots/` — offscreen renders, including the M6 acceptance
  shot of the Sleeping Giant Inn.
- `docs/formats/` — format notes written from public documentation
  (UESP wiki, NifTools spec, Microsoft docs), one file per format.
- `ARCHITECTURE.md` — the phased plan.

## Build and test

Requires a Rust toolchain (`rustup`, stable).

```sh
cargo test
```

## Milestones

- [x] M1 — BSA reader: open v104/v105 archives, list files, extract
      (zlib for v104, LZ4 for v105, embedded names supported).
- [x] M2 — Verified against a real Skyrim SE BSA from Dylan's copy
      (`Skyrim - Interface.bsa`, v105, 336 files, 336/336 extracted
      cleanly). LZ4 real-data check DONE 2026-11-13 on Skyrim -
      Textures0.bsa (625 MB, 5,889 files, 2.0 GB extracted): the first
      attempt failed on every file — v105 uses LZ4 *frame* format, not
      raw blocks — fixed the same day, now 5,889/5,889 extract
      cleanly, and all 5,889 textures decode (BC3 3,513 / BC1 2,163 /
      uncompressed RGB 213).
- [x] M3 — ESM parser: read Skyrim.esm records (TES4 header + HEDR,
      records with type/flags/form ID/fields, nested GRUPs, zlib
      compressed records, XXXX oversized fields; 7/7 synthetic
      round-trip tests pass, clippy clean). Real-data check DONE
      2026-11-13 against Dylan's Skyrim.esm (238 MB): parsed in
      3.74s, 869,687 records + 50,494 groups = 920,181, exactly
      matching the HEDR count; 44,153 compressed records
      decompressed cleanly; 118 distinct record types.
- [x] M4 — NIF mesh loader + DDS textures: NIF 20.2.0.7 (BS stream
      83/100) parses NiNode / NiTriShape / NiTriStrips / BSLODTriShape
      / BSTriShape, geometry data, BSLightingShaderProperty +
      BSShaderTextureSet, and assembles plain meshes (vertices,
      normals, UVs, tangents, indices, material name, texture paths);
      DDS parses the header + DX10 header and decodes uncompressed
      RGB(A) and BC1–BC7 to RGBA8 with mipmaps (30/30 synthetic
      tests pass, clippy clean). Real-data check against Dylan's
      meshes and textures still pending.
- [x] M5 — PEX + PSC: parse compiled Papyrus scripts (.pex) and source
      (.psc); script runtime design. `dovah-pex`: PEX header/string
      table/objects/states/functions + full opcode table (bytecode
      parsed, not executed), PSC structural parser (header, imports,
      properties, states, signatures); `docs/script-runtime-design.md`
      designs the Phase 3 runtime. 26/26 synthetic tests pass, clippy
      clean. Open questions (jump-target semantics, call operand
      order) were settled in M7.1 by disassembly against the real
      .pex files — both M7 guesses were wrong (see M7.1).
- [x] M6 — Renderer: walk Tamriel. DONE 2026-11-13. Bevy 0.20 app
      (`dovah-app`) loads a CELL from the real Skyrim.esm, resolves
      REFRs to base records, and draws the room: NIF meshes ->
      Bevy meshes and DDS textures -> Bevy images (mipmaps intact)
      when the files are present, OBND placeholder boxes at the real
      position/rotation/scale otherwise. First-person camera
      (WASD + mouse, 70° FOV, room-bounds clamp). Acceptance test
      (Dylan's pick): the Sleeping Giant Inn, Riverwood — CELL
      `RiverwoodSleepingGiantInn` (0x000133c6). Offscreen render
      verified headless on lavapipe:
      `screenshots/sleeping-giant-inn-m6.png` (1280x720, 30 frames).
      Scene: 429 objects placed — 0 real meshes, 417 placeholders,
      3 actors (red capsules: Delphine, Orgnar, Embry), 1 light, 9
      hollow room shells skipped (their OBND boxes would fill the
      room solid). Honest state: NO real meshes render yet — the
      Meshes BSA has not arrived, so every mesh file is "not in
      data"; the mesh/texture pipeline is wired and unit-tested but
      unproven on real .nif files. The room layout, positions,
      rotations, scales, and lighting are all real data. 15/15
      dovah-app unit tests pass; workspace 82/82, clippy + fmt clean.
- [x] M6.1 — Real meshes in the inn. DONE 2026-11-13. The Meshes
      BSAs landed (Meshes0 19,338 files + Meshes1 14,127 files).
      NIF parse sweep (new permanent tool
      `crates/dovah-nif/examples/nif_sweep.rs`, in-memory, nothing
      extracted to disk): 21,827 `.nif` files, 21,821 parse clean
      (99.97%); the 6 failures are all legacy BS stream 32/34
      (Oblivion-era leftovers: 4 fern/test files, 2 swords),
      correctly rejected. 53,767 drawable meshes assembled
      (23.3M vertices, 23.1M triangles). Loader fix:
      `BSDynamicTriShape` (21,132 blocks) now parses its dynamic
      section (stride from vertex-desc bits 4–7, positions from
      the first 12 bytes of each record) — validated byte-exact
      across all blocks (0 size mismatches); `meshes()` now
      returns only drawable meshes. Known gap (M6.2): 26,980
      shapes in 7,145 files have no inline geometry — skinned SSE
      meshes keep their triangles in `NiSkinPartition`, not
      parsed yet, so every character/armor/creature mesh is still
      a placeholder. Re-render: the inn now draws 403/429 objects
      as real meshes (752 sub-meshes), 23 placeholders (skinned
      hanging meat and the like), 0 mesh files missing or failed,
      0 shells skipped — the 9 hollow room shells render as real
      geometry and no longer swallow the room.
      `screenshots/sleeping-giant-inn-m6.1.png` (1280x720).
      Textures: 1 loaded, 89 missing — NOT a code bug: a probe
      over the inn's 159 models showed 91 distinct diffuse
      textures, and only `Black.dds` is in the archives we have
      (Textures0 + Textures8); the inn's wood/stone/furniture
      textures live in Textures1–7, not yet uploaded. Two render
      fixes along the way: DDS multi-mip images now build via
      `Image::new_uninit` (Bevy's `Image::new` debug-asserts
      single-mip size and panicked on the first real texture —
      4096 vs 5456 bytes), with a regression test; and the M6
      placeholder floor slab is removed now that real floor meshes
      exist (it was hiding low clutter). Actor capsules verified
      grounded (feet at z=0, shell floor at z≈−0.03 m). Headless
      render needed `mesa-vulkan-drivers` reinstalled (lavapipe
      was gone from the VM since M6). 148/148 workspace tests
      pass, clippy + fmt clean.
- [x] M7 — Papyrus runtime core. DONE 2026-11-13. VMAD parser in
      `dovah-esm` (script attachments + properties + fragments for
      INFO/PACK/PERK/QUST/SCEN, versions 2–5, objFormat 1/2):
      real-data check DONE — all 16,133 VMAD fields in Skyrim.esm
      parse cleanly (17,407 attached scripts, 46,493 properties).
      New crate `dovah-papyrus`: value model + string interning,
      frame-based execution of all opcodes 0x00–0x23, instance
      store + VMAD init (auto properties set directly, full
      properties via their setters, object properties resolve to
      instances), native registry (Debug/Utility/Game/Math/
      StringUtil + Script base; `Utility.Wait` is latent, parked
      on virtual time), time-sliced scheduler with update timers.
      `dovah-app --scripts` reports a cell's attached scripts,
      which `.pex` definitions the data folder has, and which
      instances/`OnInit` events would run. The two open questions
      (jump-target semantics, call operand order) were settled in
      M7.1 (below) — both M7 guesses were wrong; the proven
      semantics live in `dovah-papyrus/src/semantics.rs`.
      131/131 workspace tests pass, clippy + fmt clean. NOT done
      here: physics, saves, leveled lists, character pipeline —
      those are later milestones.
- [x] M7.1 — Real-script proving. DONE 2026-11-13. Dylan's
      Skyrim - Misc.bsa held the scripts: 13,494 `.pex` files
      extracted to `test-data/misc/scripts/` (one file,
      `tif__000c4428.pex`, failed to extract on a disk-quota
      error). PEX parse sweep: 13,494/13,494 parse cleanly
      (13,494 objects, 52,812 functions — 674 native — 289,644
      instructions; versions 3.1: 9,930, 3.2: 3,564). Both
      semantics questions settled by disassembly — and M7 had
      both wrong: jump targets are RELATIVE to the jump
      instruction (`target = pc + label`; 579 negative labels
      prove it, 36,362/36,362 valid), and call operands are
      METHOD-FIRST, destination-last
      (`callmethod (method, object, dest, args...)` etc.;
      10,368/10,368 static calls resolve). Three more M7 layouts
      corrected the same way (`propget`, `propset`,
      `array_findelement`). Two more real-data fixes: a PEX
      default of `None` means the type's zero value (an int
      counter starts at 0, not `None`), and `not` applies
      truthiness to any value (`if (!myRef)` tests for `None`).
      Native registry grown from what real scripts call: Math
      trig (acos/asin/atan, degree/radian conversion), Utility
      time (`GetCurrentGameTime`, `GetCurrentRealTime`,
      `IsInMenuMode`, latent `WaitGameTime`), `Debug.Notification`;
      three M7 errors corrected (`Debug.Trace` takes 2 args,
      `Utility.RandomInt`/`RandomFloat` are the real names,
      `Math.Log` does not exist). Proving pass (design doc §7.8):
      `CWScript.HoldName` 11/11 and `CraftSmithing.GetMinSkill`
      2/2 correct; all 5 Sleeping Giant Inn scripts' `OnInit`
      clean; OnInit census over all 13,494 scripts — 106 clean,
      77 errored (59 on 20 missing game-system natives, 18 on
      VMAD data the census does not provide), 0 parked, 0 stuck,
      0 type errors. Graduation: the inn's room trigger runs end
      to end and stops at the single unimplemented native
      `Quest.SetCurrentStageID`. New tools: `pex_disasm`
      (disassembler), `pex_sweep` (parse + call/jump census),
      `prove_real_scripts` (the proving pass); pinned tests in
      `crates/dovah-pex/tests/pex_real.rs` and
      `crates/dovah-papyrus/tests/real_scripts.rs`. 146/146
      workspace tests pass, clippy + fmt clean.
- [x] M8 — Android APK ("Inn Test"). DONE 2026-11-13. Signed
      debug APK, package `com.dovahengine.inntest`, arm64-v8a
      only, min SDK 26 / target SDK 34, landscape NativeActivity
      (`android_main` via `#[bevy_main]`). Loads the Sleeping
      Giant Inn from Dylan's own Skyrim.esm on the phone — the
      APK bundles NO Bethesda data. Data route (b): the app reads
      `Skyrim.esm` from the phone's Downloads folder (or its own
      app-private files dir) with MANAGE_EXTERNAL_STORAGE; if the
      file is missing or unreadable it shows a message screen
      with the exact steps instead of crashing. Route (a),
      in-app Drive download, was rejected: Drive serves 100 MB+
      files behind a virus-scan confirmation page. Touch
      controls use Dylan's own drawn buttons (21 PNGs in
      `assets/ui/`, packaged as APK assets): virtual joystick
      (moving knob) + drag-to-look on the right half, jump and
      use wired to the player controller (use = descend, a
      fly-mode stand-in until interaction exists), sprint = 3x
      speed; attack L/R, sneak, shout, map/bag/menu, quick slots
      drawn with press feedback but inert; bars full, compass
      fixed. APK: `dovahengine-inn-test.apk` (~406 MB — debug
      build, debug symbols stripped; a release build would be
      far smaller but release builds of dovah-app currently fail
      on this box). Playtested by Dylan on his S25 Ultra
      2026-11-13: it works — the inn renders, his buttons are all
      in place (`screenshots/first-phone-run-m8.jpg`). Three fixes
      queued for M8.1 (next build, with the real meshes):
      immersive fullscreen (status/gesture bars overlap the HUD),
      compass yaw (strip must rotate with the camera), stamina
      tied to sprinting (health/magicka stay full — no
      combat/magic drivers yet). First load shows a black screen for ~10 s while the
      238 MB plugin parses (no loading screen yet). Workspace
      136/136 tests pass, clippy + fmt clean. Desktop headless
      screenshot path still works after the lib/bin split
      (`screenshots/sleeping-giant-inn-m8.png`).
- [x] M8.1 — Second Inn Test APK (v0.2.0). DONE 2026-11-14.
      Signed debug APK, same package `com.dovahengine.inntest`
      (installs over M8's v0.1.0). Real meshes from M6.1 now
      load on the phone: the APK reads Meshes0/1.bsa from
      Downloads alongside Skyrim.esm (403 real meshes, 752
      sub-meshes in the inn). Dylan's three playtest fixes:
      (1) immersive fullscreen — status + navigation bars
      hidden via NDK window flags + JNI `setSystemUiVisibility`
      (immersive-sticky), re-applied on focus regain (winit's
      `set_fullscreen` is a no-op stub on Android and cargo-apk
      has no theme support — JNI was the only route); (2)
      compass rotates with camera yaw — the frame is Dylan's
      drawing with the baked N and diamond painted out,
      letters/ticks ride on a strip that slides with heading,
      diamond fixed at centre; (3) stamina drains 10/s while
      sprinting, regens 12/s otherwise (health/magicka stay
      full — no combat/magic drivers yet). Plus gravity +
      grounded walking (his "floaty, no-clip" complaint): the
      player falls to the floor and stays grounded, Space/touch
      Jump is a real jump (~1.1 m), USE is inert until
      interaction exists; ground comes from a 1 m floor grid
      built from the real meshes at scene build (1,053 cells
      with data in the inn), with the room floor as fallback —
      full wall/furniture collision is still out of scope.
      Loading screen: the ESM parse runs on a worker thread
      behind a "Loading the Sleeping Giant Inn..." screen
      instead of ~10 s of black. Textures on the phone: 16 of
      the inn's 90 diffuse textures resolve from Textures0+1+8
      (was 1 of 90 from Textures0+8); the other 74 live in
      Textures2–7. BC7 real-data check still open: Textures1
      (885 files) is BC3/BC1 only, zero BC7. Desktop textured
      screenshot: `screenshots/sleeping-giant-inn-m6.1-textured.png`.
      Downloads for Dylan: Skyrim.esm (238 MB) + Meshes0
      (1.1 GB) + Meshes1 (356 MB) + Textures0 (625 MB) +
      Textures1 (1.4 GB) = about 3.7 GB. APK:
      `dovahengine-inn-test.apk` (428,750,825 bytes, ~409 MB),
      apksigner-verified. 159/159 workspace tests pass, clippy
      + fmt clean. Gravity/immersive/compass/stamina are
      unproven on the phone until Dylan playtests.
- [x] M8.1.1 — Crash fix APK (v0.2.1). DONE 2026-11-14.
      v0.2.0 crashed immediately on launch on Dylan's S25 Ultra
      ("this app has a bug", before any frame). Cause: the M8.1
      immersive code called `AndroidApp::set_window_flags` — an
      NDK call that touches the Activity's Window — directly
      from the Rust thread. Once the window is attached,
      `Window.setFlags` reaches `ViewRootImpl`, which throws
      `CalledFromWrongThreadException` off the UI thread; the
      exception stays pending on the Rust thread and ART aborts
      the process at the next JNI call. Fix: every window call
      now runs on the Java UI thread (`Window.setFlags` via JNI
      in the same UI-thread closure as `setSystemUiVisibility`);
      the closure is wrapped in `catch_unwind` (android-activity
      aborts on a panic in the looper callback); pending JNI
      exceptions are described and cleared before returning
      (jni-rs reports them but leaves them pending); null
      VM/activity/window/decor bail out with a log. Also
      hardened: the load-channel lock now recovers from
      poisoning instead of `expect`. Same package, versionName
      0.2.1, installs over v0.2.0. APK: `dovahengine-inn-test.apk`
      (428,759,017 bytes, ~409 MB), apksigner-verified (v2+v3).
      161/161 workspace tests pass, clippy + fmt clean.
- [x] M8.1.2 — Instrumented crash-fix APK (v0.2.2). DONE 2026-11-14.
      v0.2.1 STILL crashed on Dylan's phone — the JNI theory was
      wrong. Decisive evidence from Dylan: with file permission
      DENIED the app showed the message screen fine (Bevy init,
      JNI immersive mode, message UI all work); with permission
      GRANTED, instant crash before the loading screen. The real
      cause, proven from the Bevy 0.20 source: the M8.1 loading
      screen inserts `RoomBounds` only when the worker thread
      delivers the cell data, but `keyboard_move` required
      `Res<RoomBounds>` from the first Update tick — a missing
      required resource is a validation failure, and Bevy's
      default error handler panics on it; android-activity's
      `abort_on_panic` turns that into SIGABRT before the first
      frame. Fix: `keyboard_move` takes `Option<Res<RoomBounds>>`
      (like the `FloorGrid` next to it) and returns early until
      the bounds exist; regression test runs the player plugin
      with no `RoomBounds` and no player entity. Instrumentation
      (so the next crash is read, not guessed): a startup-stage
      log at `Downloads/dovahengine-startup.log` — one
      timestamped line per stage, open-append-close per line,
      previous run rotated to `dovahengine-startup-prev.log`,
      panic hook installed first in `main()` (message + location
      + backtrace), RSS at stage boundaries, first-frame marker
      (tick 2 proves frame 1 rendered), message screen shows the
      previous log's tail, and a `dovahengine-no-immersive` flag
      file in Downloads skips the JNI path. Defensive caps:
      FloorGrid total cells (cell size grows past 100k cells)
      and per-cell heights (4,096), single-mesh vertices/indices
      (1M/3M — skipped, not allocated). Same package,
      versionName 0.2.2, installs over v0.2.1. APK:
      `dovahengine-inn-test.apk` (428,849,129 bytes, ~409 MB),
      apksigner-verified (v2+v3). 169/169 workspace tests pass,
      clippy + fmt clean. The log itself was proven end to end
      on the desktop real-data path (full stage lines + RSS +
      a captured panic with backtrace); the desktop screenshot
      re-check was blocked because mesa-vulkan-drivers is no
      longer installed on this VM (no GPU adapter found).
- [x] M8.1.3 — Startup-order crash-fix APK (v0.2.3). DONE 2026-11-14.
      v0.2.2 ALSO crashed — but the startup log caught it
      red-handed, both runs identical: `init_state::<AppState>()`
      panicked at `bevy_state-0.20.1/src/app.rs:102` ("The
      `StateTransition` schedule is missing") during app
      construction, before `app.run()`. Cause: `run()` called
      `init_state` BEFORE `add_plugins(DefaultPlugins)`, and the
      `StateTransition` schedule only exists once `StatesPlugin`
      (inside `DefaultPlugins`) registers it. The RoomBounds fix
      was real but was the SECOND crash hiding behind this FIRST
      one. Fix: `DefaultPlugins` before `init_state`, and the
      windowed app is now built by exactly one function,
      `build_windowed_app` (shared resources split into
      `insert_shared_resources`), used by `run()` and by the new
      regression test. Why desktop missed it: every desktop run
      used `--screenshot`, which takes a branch that never calls
      `init_state` — the windowed branch (the one Android takes)
      was never executed on a display-less machine. The test
      closes that gap: it builds the app the way `run()` does
      and runs two updates in the Loading state, with a headless
      plugin set (`MinimalPlugins` + `StatesPlugin` +
      `InputPlugin` — `DefaultPlugins` cannot run in a test:
      winit refuses an event loop off the main thread,
      `RenderPlugin` wants a GPU, `CorePipelinePlugin` loads a
      `Shader` at build time). The test FAILS on the old order
      with the exact phone panic (same file:line) and PASSES on
      the new one. Writing it found a THIRD latent crash in the
      same path: `poll_load` had two `&mut Text` queries
      (`With<LoadingTitle>` / `With<LoadingText>`) that Bevy
      cannot prove disjoint — B0001 panic at system init on the
      first Update tick; fixed with `Without` filters. Same
      package, versionName 0.2.3, installs over v0.2.2. APK:
      `dovahengine-inn-test.apk` (428,865,513 bytes, ~409 MB),
      apksigner-verified (v2+v3). 170/170 workspace tests pass,
      clippy + fmt clean.
