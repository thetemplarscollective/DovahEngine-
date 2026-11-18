# ChatGPT Brief — Rendering & Presentation Milestone (2026-11-17)

Source: ChatGPT ("GPT Sofia"), from the v0.4.13 source zip Dylan sent.
Share link: https://chatgpt.com/share/6afceb1c-7f00-83ea-b3dc-36cdd0447827
Dylan adopted the structure 2026-11-17 19:02 EST. This file is the plan of
record; Jarvis's corrections are at the bottom.

## The four milestones

1. **Lighting calibration** — make Skyrim look right with the assets installed.
   No texture upgrades, no advanced lighting yet.
2. **Original Skyrim UI** — the real Scaleform menus via Ruffle, plus the
   high-resolution UI fix.
3. **Loading performance** — BSA indexing, caching, parallel decode.
4. **Game presentation** — the original main menu and loading screens.

Priority order: lighting calibration → UI resolution → Ruffle compatibility
→ loading architecture → main menu and loading screens.
Overall: correct rendering first, original interfaces second, performance
third, presentation polish fourth.

## Phase 1 — Lighting Calibration (Milestone 1)

Investigate `crates/dovah-app/src/scene.rs`. Keep the existing lighting
model; calibrate it, do not replace it.

- Audit the Skyrim LIGH record → Bevy point-light conversion (position,
  radius, fade, intensity, color).
- Investigate the overly bright Sleeping Giant Inn interior: are the
  fireplace and candle lights overlapping too strongly? Why does the
  ambient fill wash the room out? Compare floor, ceiling, NPC faces and
  shadowed corners separately.
- Investigate the washed-out exterior and vegetation: verify directional
  sunlight and the sky ambient contribution; check whether exterior
  ambient and sunlight double-light surfaces; check material base colors
  are multiplied correctly.
- Audit shadows: bias, normal bias, map resolution (currently 512), the
  4 shadow-light slots and whether they are used efficiently, shadows
  disappearing when the camera moves.
- Compare reference screenshots against engine output (vanilla inn and
  Riverwood references in `gameplay/reference/`).

Current values (v0.4.13): sun 12,000 lux; interior ambient 100, warm amber;
exterior ambient 2,400; max 4 shadow lights; point-light shadow maps 512.
Controlled tests wanted: interior daylight-free lighting, fireplace/candle
lighting, NPC facial lighting, exterior daylight, shadowed vegetation.

## Phase 2 — Original Skyrim UI (Milestone 2)

Investigate `ruffle.rs`, `inventory.rs`, `pause.rs`, `touch.rs`,
`vendor/ruffle`. Use the real Interface assets from the installed BSAs.

Priority menus: inventory, journal, dialogue, magic, map, main menu,
loading interface. Source files: startmenu.swf, loadingmenu.swf,
hudmenu.gfx (already live via Ruffle), inventorymenu.swf,
quest_journal.swf, dialoguemenu.swf, magicmenu.swf, mapmenu.swf,
statsmenu.swf.

Ruffle compatibility items named: nested ImportAssets2, character ID
collisions, DefineExternalImage2, missing translations, external image
resolution, interface archive override priority. Do not replace working
original interfaces with approximated Bevy interfaces unless necessary.

## Phase 3 — High-Resolution UI (Milestone 2)

The Ruffle HUD caps at 1280×720 (`HUD_MAX_WIDTH`/`HUD_MAX_HEIGHT` in
ruffle.rs) and picks its size from the screenshot target or window size —
the suspected cause of the blurry phone UI. Investigate:

- Whether Bevy reports logical window dimensions instead of the physical
  framebuffer dimensions.
- Whether the Ruffle output is upscaled to fill the screen.
- Whether the HUD texture is filtered/sampled at an unsuitable resolution.
- Whether the UI renders below the 3D scene's resolution.
- Whether the SWF vectors rasterize at the correct resolution.

Target: preserve Skyrim's 1280×720 design coordinates, but rasterize the
interface at the phone's native resolution. Do NOT simply increase UI
scale (that makes elements larger, not sharper). Inventory and pause menu
should use the original Skyrim fonts and artwork where possible, not the
fallback Bevy font.

## Phase 4 — Asset Loading Optimization (Milestone 3)

Investigate `assets.rs`, `scene.rs`, `lib.rs`, and the BSA/NIF/DDS crates.

- Persistent BSA archive indexing (avoid re-indexing on launch).
- Correct Skyrim archive override priority across the whole provider —
  `read_file()` is first-archive-wins while `read_file_latest()` scans in
  reverse, so older Interface assets can beat newer Patch assets.
- Caching of decompressed assets; mesh caching (reuse parsed NIFs).
- Background NIF parsing; parallel DDS decoding.
- Lazy loading + preloading nearby doors/NPCs/environment.
- Bounded memory cache with eviction. Do NOT preload the whole game.
- Non-blocking scene construction; batched GPU texture uploads.
- Measure startup, first-area load, and door-transition time separately.

Preferred pipeline: game data → asset index/priority resolver → workers
(NIF / DDS / ESM / interface) → asset cache → Bevy renderer.

## Phase 5 — Original Main Menu (Milestone 4)

`startmenu.swf` + dependencies. A dedicated MainMenu application state,
not an overlay on the loaded world. Items: New Game, Continue, Load Game,
Settings, Credits, Quit — original artwork, logo, layout, typography,
animations.

## Phase 6 — Original Loading Screens (Milestone 4)

`loadingmenu.swf` + dependencies. Original artwork, lore text and gameplay
tips, rotating 3D item/character models where supported, real loading
progress, destination-specific info. Evolve the existing loading screen
in `lib.rs` (DovahEngine logo + text) rather than replacing it.

## Performance (cross-cutting)

7–16 fps on the S25 Ultra, Ruffle CPU readback 37–49 ms early in a scene.
Investigate whether Ruffle can avoid full CPU framebuffer readback and
render through a GPU-shared texture instead — likely a bigger win than
mesh optimizations. Do not spend all the effort on loading while ignoring
the frame-time problem.

## Acceptance criteria

No regression in Riverwood exterior or the inn. Interior lighting balanced
with depth. Exterior no longer washed out. UI crisp at native resolution.
Original interface assets render correctly. Main menu and loading screens
use the original assets. Loading never freezes the render thread. Memory
controlled. Gameplay and touch controls keep working.

## Development rules

Work incrementally. Before changing code: inspect the implementation,
identify the root cause, explain the proposed fix, identify affected
systems. After: compile, run relevant tests, capture before/after renders,
measure performance, report regressions and remaining limits. No unrelated
changes, no rewrites of functioning systems.

## Jarvis's corrections (2026-11-17, against the actual tree)

1. **Nested ImportAssets2 is NOT a blocker.** It was fixed in M3 by patch
   0005 (upstream ruffle-rs/ruffle issue #22426). The feasibility doc's
   blocker section is stale on this point; the M3 gate passed and full
   menus are reachable in the harness. The remaining Ruffle/M5 work is:
   tag-1009 keycap art, `$` translation tokens, the 3D item model in the
   card (UpdateItem3D), fontlib production shaping, safeRect phone inset
   values. Do not re-investigate the import gate.
2. **New Game stays UNWIRED.** Dylan's standing decision (2026-11-16):
   New Game appears on the menu but is not wired or used — no new-game
   flow, no character creation. The brief's Phase 5 lists New Game as a
   working item; that part is NOT adopted.
3. **The Bevy UI is a deliberate stand-in**, kept per-screen until the
   matching Ruffle screen reaches parity (M6 cutover). It is not a mistake
   to be removed wholesale.
4. The Ruffle GPU-compositing investigation is the M6 item; the revised
   assessment is that capture/readback is NOT the top Ruffle cost on
   desktop (per-step render dominates), but the phone logs prove capture
   dominates on the phone early in a scene — the phone data governs.
