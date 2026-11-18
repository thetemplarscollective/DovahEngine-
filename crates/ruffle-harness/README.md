# ruffle-harness (Ruffle M1 + M2 + M3 + M4)

Standalone harness crate: renders Skyrim's real `hudmenu.gfx` and
`inventorymenu.swf` offscreen with Ruffle, driven by stub game data.
Outside the engine's hot path — no Bevy coupling, its own wgpu device
(`WgpuRenderBackend<TextureTarget>::for_offscreen`).

## Usage (from the workspace root)

```sh
./target/debug/ruffle-harness [swf] [fontlib] [out-dir] [total-frames] [mode]
```

Defaults: `interface-extract/study/hudmenu.gfx`,
`interface-extract/tools/ruffle-test/gfxfontlib.swf`, `screenshots`, 60 frames,
`demo` mode. Modes: `demo` (the stub-data drive), `probe` (demo drive +
jump the meter/location clips to mid-timeline frames), `loc`
(`SetLocationName` only), `jump` (`InitExtensions` + probe jumps only, no
`Set*` calls), `force` (FORCED percents 25/50/90, bypassing the
`onEnterFrame` → `Update()` tween), `upd` (unforced percents + 10 direct
`Update()` calls on each Meter), `inventory` (the inventory-menu drive —
see below; pass the inventory SWF as `[swf]`). The binary also prints a
clip-state dump (`STATE_PATHS` — frames, positions, visibility, Meter
percents — or `INV_STATE_PATHS` in `inventory` mode) after the drive and
after the drive frames, and `getDepth` path checks in probe/jump modes.

The harness does NOT create `out-dir` — `mkdir -p` it first. Set
`RUST_LOG=debug` (or any `target=level` / level value) for Ruffle's
`tracing` output on stderr — loader/preload/import lines, AVM1 warnings,
the lot; off by default.

Outputs in `out-dir`:

* `ruffle-m1-hud-nodata.png` — HUD after 10 settle frames, no game data.
  Matches the study's `shot-hud-fontlib.png` (real compass, "Location",
  subtitle lines, real font).
* `ruffle-m1-hud.png` — HUD after the stub-data drive. The M1 proof artifact.
* `ruffle-m1-seq/seq-NNN.png` — sequence frames every 15 steps.

## Layout

* `src/provider.rs` — `StubProvider`, the `ExternalInterfaceProvider`: logs
  every SWF→game call; answers `GetButtonFromUserEvent` with canned PC
  keycaps, and `RequestItemCardInfo` with a canned Iron Dagger object
  (type 2, damage 4, value 10, weight 2.5 — the fields `ItemCard.as`
  reads). The response MUST be synchronous — `GameDelegate.call` deletes
  `responseHash[uid]` right after `ExternalInterface.call` returns, so the
  provider invokes the `"respond"` callback DURING `call_method` (the
  re-entrant pattern from Ruffle's own test suite).
* `src/navigator.rs` — `MemoryNavigatorBackend`: serves the synthesized
  `gfxfontlib.swf` from memory (URL match on the last path segment);
  everything else falls through to Ruffle's `fetch_path` disk helper.
  `base_path` is canonicalized at construction — `Url::from_file_path`
  refuses relative paths, so a relative base made every component import
  fail with "relative URL without a base" (M3: the inventory menu stayed
  pure white until this was fixed; the fontlib only worked because the
  memory intercept bypasses URL resolution entirely).
* `src/runner.rs` — `Harness`: builds the player, steps frames
  (`preload` + `run_frame` + `render`, timed), captures via downcast to
  `WgpuRenderBackend<TextureTarget>`. Four host→SWF channels:
  `call` (GameDelegate `"call"` callback), `call_root_function` (frame-1
  globals such as `InitExtensions`), `call_clip_function` (dotted path,
  e.g. `HUDMovieBaseInstance.Health`), `set_clip_variable` (dotted path +
  variable name, e.g. `Menu_mc.InventoryLists_mc.ItemsList` /
  `entryList` — Scaleform's `SetVariable`, patch 0006). Plus diagnostics:
  `clip_depth` (`getDepth` path check) and `clip_state` (vendored
  `Player::clip_state` — frames, positions, visibility, Meter percents).
* `src/main.rs` — CLI driver: settle → no-data capture → drive → driven
  capture → step to `total-frames`, then prints timing summaries and the
  provider call log. Prints the clip-state dump after the drive and after
  the drive frames.

## The drive (`drive_hud_demo`)

1. `InitExtensions` via `call_root_function` — registers every GameDelegate
   callback. Without it the bridge never starts (see below).
2. `SetHealthMeterPercent(75, false)`, `SetMagickaMeterPercent(60, false)`,
   `SetStaminaMeterPercent(90, false)`, `SetCompassAngle(0, 0, true)`,
   `SetLocationName("Riverwood")`, `SetCrosshairTarget(true, "Delphine", …)`
   (6 args — an NPC has no weight/cost), `ShowTutorialHintText("[Activate]
   Talk to Delphine", true)` — the bracket token makes the SWF call
   `GetButtonFromUserEvent`, proving the SWF→game round trip.

   Percents are 0–100, NOT 0–1 fractions: `Components.Meter` clamps to
   0–100 and lerps the bar clip's Empty→Full frames over 0–100. M1 passed
   0.75/0.6/0.9, which the SWF read as ~0% (M2 fix).

## The inventory drive (`drive_inventory_demo`, M3)

Run with mode `inventory` and the inventory SWF:

```sh
mkdir -p screenshots/ruffle-m3-inventory2
./target/debug/ruffle-harness \
  interface-extract/tools/ruffle-test/fontlib-test/inventorymenu.swf \
  interface-extract/tools/ruffle-test/gfxfontlib.swf \
  screenshots/ruffle-m3-inventory2 60 inventory
```

1. `InitExtensions` via `call_root_function` (registers the GameDelegate
   callbacks — same as the HUD).
2. `SetCategoriesList` with 10 categories as (text, flag, bDontHide)
   triples: All (511), Weapons (1), Apparel (2), Potions (4), Scrolls
   (8), Food (16), Ingredients (32), Books (64), Keys (128), Misc (256).
3. `set_clip_variable("Menu_mc.InventoryLists_mc.ItemsList",
   "entryList", …)` — 5 items (`{text, count, filterFlag, enabled}`):
   Iron Dagger, Iron Sword, Leather Armor, Minor Healing (3), Bread (2).
   The game injects items this way in Scaleform; there is no GameDelegate
   handler for it.
4. `InvalidateListData` — re-derives visible categories from the items.
5. `ShowItemsList(false, true)` on `Menu_mc.InventoryLists_mc` — opens
   the items panel (no sound, with the slide-in animation). The
   `showItemsList` event fires synchronously: the SWF calls
   `UpdateItem3D(true)` + `RequestItemCardInfo(2)`, the provider answers
   with the canned Iron Dagger, and the card fades in showing
   IRON DAGGER / DAMAGE 4 / WEIGHT 2.5 / VALUE 10.
6. `UpdatePlayerInfo` — armor 0, damage 4, encumbrance 45/300, gold 127,
   health/magicka/stamina 100/100 (colors omitted → default white).

Layout note: the components live under the `Menu_mc` clip the root
places (char 91, `InventoryMenuObj`) — NOT on the root. The card sits
one level deeper, inside `ItemCardFadeHolder_mc`. Paths:
`Menu_mc.InventoryLists_mc.{ItemsList,CategoriesList}`,
`Menu_mc.ItemCardFadeHolder_mc.ItemCard_mc`,
`Menu_mc.BottomBar_mc`. (The first M3 drive run failed on this — every
path NOT FOUND until the tag parse showed the root places exactly one
clip, `Menu_mc`.)

## Why Ruffle is vendored (`vendor/ruffle/`)

Stock Ruffle 0.7.1's public API cannot invoke a root-timeline function, and
Skyrim's menus register their GameDelegate callbacks *inside* `InitExtensions`
(a frame-1 global the host must invoke). `call_internal_interface` only
reaches SWF-registered callbacks — chicken-and-egg. The `avm1` / `avm2` /
`display_object` modules are private, so no external crate can work around it
via `mutate_with_update_context` either (the feasibility study's
"fiddly-but-feasible" variable-injection claim does not hold for an external
crate). The vendor tree is 0.7.1 + four patches (see
`vendor/ruffle/README-DOVAHENGINE.md`): `0001-call-root-function`
(`Player::call_root_function` / `call_clip_function`),
`0002-clip-state` (diagnostic `Player::clip_state`),
`0003-stage-visible-rect` (Scaleform `Stage.visibleRect`/`safeRect`),
`0004-goto-float-frame` (`gotoAndStop`/`gotoAndPlay` truncate float
frame numbers, as Flash does), `0005-nested-imports` (recursive
preload + import-wins ID registration — the #22426 fix),
`0006-set-clip-variable` (`Player::set_clip_variable`, Scaleform's
`SetVariable`). 0001, 0003, 0004, 0005, and 0006 are genuine upstream
gaps/divergences worth proposing to ruffle-rs/ruffle; 0002 is
diagnostic-only. Upstreaming is future work (not done in M3).

Build note: `ruffle_core`'s build script needs `java` on PATH
(`$HOME/workspace/jdk17/bin`) to build `playerglobal_avm2`.

## Status (2026-11-16, M2 DONE)

PROVEN (M1):

* Real HUD renders from the unmodified `.gfx` + synthesized fontlib
  (compass, "Location", subtitle lines, real Futura).
* `InitExtensions` runs via `call_root_function` — GameDelegate callbacks
  `"call"` + `"respond"` register (provider log shows both).
* GameDelegate drive lands: crosshair + "Delphine" target name, compass N,
  tutorial hint text all render from the drive.
* Full SWF→game round trip: `GetButtonFromUserEvent("Activate")` → provider
  answers `"E"` synchronously via `"respond"` (1 call, 1 respond in the log).

PROVEN (M2) — meters + location now render:

* **Root cause 1 (primary): Ruffle has no Scaleform `Stage.visibleRect` /
  `safeRect`.** The HUD's `GlobalFunc.Lock` reads both to anchor the
  meters, compass, and location name to the stage edges. Undefined, `Lock`
  computed NaN anchors; Ruffle's `globalToLocal` coerces NaN to 0 twips
  (Rust `as i32` saturating cast), so every anchored clip parked at global
  (0, 0). The meter art hangs ABOVE its clip origin, so it rendered off
  the top edge; the location text landed off the left edge (its parent at
  x=0, the text at local x=-439). Only the compass (art below its origin)
  stayed visible. Fixed by `0003-stage-visible-rect.patch` — both
  getters return the full stage rect. After the fix: Health/Magica/
  Stamina at global y=720 (bottom), Stamina at x=1280 (right),
  LocationLockBase at (1280, 0) (top-right).
* **Root cause 2 (drive bug): percents were 0–1 fractions; `Meter`
  expects 0–100.** M1 passed 0.75/0.6/0.9, read as ~0%. Fixed in
  `drive_hud_demo` (75/60/90). Real but not sufficient — the demo still
  showed nothing with corrected percents until 0003 landed.
* **Root cause 3 (latent, found via the state dump): Ruffle's
  `gotoAndStop` ignored non-integer frame numbers.** `Meter.Update`
  passes raw `Lerp` floats to `gotoAndStop` (only `SetPercent` floors);
  stock Ruffle's `goto_frame` only ran a direct goto `if n.fract() ==
  0.0`, so the float fell through to the label branch and silently
  no-op'd. The bars tweened their percents correctly (`CurrentPercent`
  100→75) but the bar clip never moved off its Full frame. Fixed by
  `0004-goto-float-frame.patch` (truncate, as Flash does). After the
  fix: HealthLeft lands at frame 50 for 75% (Lerp(200,1,0,100,75) =
  50.75 → 50); the demo frame shows 75/60/90% fills.
* Verified end to end: the demo frame shows compass N, "Riverwood"
  top-right, crosshair + "Delphine" + hint, and all three bars at the
  bottom at their driven fills. Pixel measurements (driven frame, before
  → after M2): meter strip (y 600–720) mean 51.0 → 52.6, non-background
  pixels 0 → 16,306; location corner mean 51.0 → 52.8, non-background
  pixels 0 → 1,303. Proof: `screenshots/ruffle-m2-demo/ruffle-m1-hud.png`
  (viewed); `screenshots/ruffle-m2-jump/` shows full bars + default
  "Location"; `screenshots/ruffle-m2-loc/` shows "Riverwood" alone.
* Ruled out along the way (all with evidence): timeline-position fault
  (probe jumps landed — `_currentframe` 100/85 confirmed via the state
  dump; frame 100/85 are mid-hold at full alpha per the timeline dump),
  `Lock` as a no-op (it moves clips — to the WRONG place), label
  `gotoAndStop` (works — Compass at "Zero" = frame 1, bar `Empty=200` /
  `Full=1` read correctly), the mask (the unmasked meter background was
  also missing), `SetText` (text field reads `text=Riverwood`),
  `onEnterFrame` (fires — `CurrentPercent` reaches its target over the
  drive frames).

Known gaps (from the study, confirmed in M1): `$` translation tokens render
literally (`$WEIGHT`); keycap art does not render (`BitmapData.loadBitmap`
of tag-1009 images — the `[Activate]` token stays literal in the hint text).

## Status (2026-11-16, M3 DONE) — the nested-import gate PASSED

Approach (a), FIX RUFFLE, won. Approach (b), build-time SWF surgery, was
NOT implemented — unnecessary once the Ruffle fix landed. The inventory
menu renders with all three components in real fonts, driven with stub
data. Proof: `screenshots/ruffle-m3-inventory2/ruffle-m1-hud.png`
(viewed) — categories (All/Weapons/Apparel/…), items (Iron Dagger,
Iron Sword, Leather Armor, Minor Healing (3), Bread (2)), the Iron
Dagger card, and the bottom bar (Damage 4, Carry Weight 45/300,
Gold 127, red health bar). Pixel counts (non-background pixels):
baseline blank 0 → undriven patched 220,789 → driven 446,209.

What was wrong, in order:

* **Harness bug first (not Ruffle):** the baseline inventory run stayed
  pure white — worse than the study's blank, frame 1 never ran. The
  harness passed a RELATIVE `base_path` to `MemoryNavigatorBackend`;
  `Url::from_file_path` refuses relative paths, so every component
  import failed with "relative URL without a base" and the importer's
  `awaiting_import` stayed set forever (stock `load_asset_movie`
  never calls `finish_importing` on the error path). The fontlib only
  worked because the memory intercept bypasses URL resolution. Fix:
  canonicalize `base_path` at construction (+ regression test).
* **Then the real #22426:** with navigation fixed, the log showed the
  exact upstream failure — ItemCard/InventoryLists/BottomBar preload
  `false` (stopped at their nested fontlib import), the nested fontlib
  load completed but nobody resumed the component preload, and frame 1
  then failed with "Tried to instantiate a non-registered character
  ID 87/89/90".
* **The collision underneath it:** the font registrations collided
  with the ROOT's exports (`import_exports_of_importer` copies the
  importer's exports into the component library under raw IDs — the
  root's `360_RB` at ID 37 occupied the slot ItemCard's
  `$EverywhereMediumFont` import needed). Font lookup is strictly by
  ID with no fallback, so the collision was a hard failure for
  component text, not cosmetic. And deferring nothing, the components'
  OWN later defines (IDs 38+) collided with the copied root exports
  too — 65 lost shapes on the first patched run.
* **The fix — `0005-nested-imports.patch`** (loader.rs + library.rs +
  display_object/movie_clip.rs): `load_asset_movie` preloads imported
  movies to completion, resuming after each nested import (hand-rolled
  `YieldOnce` future, no new dep, wasm-safe; 10,000-resume cap with a
  warning); a failed import now un-wedges its importer;
  `import_exports_of_importer` runs only at preload completion;
  imported assets win their ID via `register_character_overwrite`.
  Result: all 11 preloads `true` (8 fontlib + 3 components), 0
  collisions, 0 non-registered IDs.
* **Variable injection — `0006-set-clip-variable.patch`** (player.rs):
  `Player::set_clip_variable(path, name, value) -> bool`, Scaleform's
  `SetVariable`. The inventory's items arrive only by injection
  (`ItemsList.entryList`), so this was required for the drive — and
  it is the API M5 needs for every menu's lists. Same dotted-path
  walk and `into_avm1` conversion as `call_clip_function`.
* **Regression check:** the HUD re-render after all Ruffle changes is
  pixel-identical to the M2 proof (sha256 `c21cbe7d…`, mean abs diff
  0.0000). Ruffle's own import regression tests
  (`tests/tests/swfs/import_assets/`: avm1_imports_avm1 — the #22426
  minimal-repro shape — avm1_non_swf_import, empty_url) pass 3/3
  with the patch (`cargo test -p tests --test tests -- import_assets`,
  0.45s). The tests were not run pre-patch, so this is a
  no-regression signal, not a before/after — the before/after is the
  inventory menu itself.

Deliberately left out of M3: tag 1009 keycap art (still green/blue
blocks in the driven screenshot — the plan said to do it in M3, but
the gate was the import fix and 1009 is a separate render-path job
for M5); `$` translation tokens (still literal `$Damage`/`$Equip`/…);
fontlib production shaping; `safeRect` phone insets; the 3D item
model in the card (the SWF asks via `UpdateItem3D` — engine work for
M4/M5). Known nits: 83 deterministic `Avm1::pop: Stack underflow`
warnings during the components' class-init preload (a Ruffle AVM1
quirk with Scaleform-compiled class definitions — the popped value
is discarded anyway; the menu works fully despite them; they appear
only now because the components' code finally executes); the
bottom-bar buttons overlap the last category rows slightly; one
unexplained `PlaySound("UIMenuBladeOpenSD")` per run (the menu's own
blade sound — harmless).

## Status (2026-11-16, M4 DONE) — the HUD is live in the engine

M4 made this crate a LIBRARY the engine depends on (`dovah-app`
`Cargo.toml`: `ruffle-harness = { path = "../ruffle-harness" }`) —
the bridge is one copy, not two. The harness binary stays as the
offline test rig for M5 menu work.

* `Harness::new_shared(SharedWgpu, swf, fontlib, name, base_path,
  w, h, fps)` — builds the player on a caller-supplied wgpu device
  (Bevy's, in the engine) via `Descriptors::new` +
  `TextureTarget::new` + `WgpuRenderBackend::new`. The existing
  `Harness::new` (offscreen device, used by the binary) and the
  new `new_from_bytes` all funnel through `from_parts` — one
  executor, one navigator, one provider, one player-builder path.
* `Harness::set_transparent()` — `player.set_window_mode
  ("transparent")`, so the 3D scene shows through the HUD texture.
  The harness binary does NOT call it (its captures keep the white
  background).
* `Harness::set_safe_area_insets(l, t, r, b)` — patch 0007 (see
  `vendor/ruffle/README-DOVAHENGINE.md`): the `safeRect` host
  setter M2 noted the phone build needs. Desktop passes (0,0,0,0).
* `src/fontlib.rs` — `synthesize_fontlib(fonts_en_swf)`: builds
  `gfxfontlib.swf` at runtime from the game's `fonts_en.swf`
  (Patch.bsa's copy: 16 fonts at shifted IDs — synthesis maps by
  (font name, bold flag), never by ID). Byte-identical to the
  study's fontlib when fed the Interface fonts. `examples/
  synthesize_fontlib.rs` is the CLI for the same function (used
  for the M4 de-risk check: the Patch fontlib renders the HUD
  byte-identically to the M2 proof, sha256 `c21cbe7d…`).
* `Value` and `SharedWgpu` are re-exported from the crate root so
  the engine can build call args and hand over Bevy's handles
  without a direct `ruffle_core` dependency.
* The engine side lives in `crates/dovah-app/src/ruffle.rs` (the
  `RuffleHudPlugin`, the main-world drive, the render-world
  player). The one real M4 defect was on the engine side, not
  here: the extract system used plain `Res<T>` (reads the RENDER
  world) instead of `Extract<Res<T>>` (reads the MAIN world) —
  see `docs/flash-interface-feasibility.md` §M4 and the comment
  on `extract_ruffle_hud`.
* Tests: 18/18 (5 fontlib tests added in M4). Clippy 0, fmt clean.

## Measurements (this box, debug, jobs=2, lavapipe)

* First Ruffle build (git 0.7.1): ~14 min of compiling across two runs
  (9m06s to the java failure + 5m04s to the harness's own errors).
* Vendored rebuild (Ruffle crates only): 3m17s first time; incremental
  `ruffle_core` rebuild after a patch: ~30–45 s. Incremental harness
  rebuild: ~6–30s. (Wall-clock figures are polluted by lock waits behind
  other agents' builds; the compile portions above are the real numbers.)
* Debug binary: 421,011,256 B (~402 MB, with debug info) at M3.
* Frame times (60-frame runs, steady-state): AVM1+logic (`run`) mean
  16–24 ms, p95 50–85 ms; `render` mean 97–147 ms (lavapipe software
  rasterization + readback — NOT representative of a real GPU).
  Player build: ~1–3 s. Total 60-frame run: ~15–28 s wall.
* Inventory menu (60-frame driven run, M3): `run` mean 12.7 ms steady
  state, `render` mean 56.7 ms — the menu is cheaper than the HUD
  (fewer tweens, no compass rotation).
