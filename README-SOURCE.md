# Dovahengine — Source Code

A clean-room Skyrim Special Edition engine, written from scratch in Rust +
Bevy 0.20. It reads game data at runtime from the player's own legally owned
copy of Skyrim SE. It is NOT Mudcrab, NOT an OpenMW fork, and contains no
Bethesda code or assets. Target: an Android APK (Samsung Galaxy S25 Ultra).

Current version: **v0.4.13** (2026-11-17). The acceptance test is the Sleeping
Giant Inn in Riverwood (CELL 0x000133c6) and the Riverwood exterior.

## Layout

- `crates/dovah-app/` — the engine: renderer, scene build, player, dialogue,
  inventory, save system, touch UI, Ruffle HUD integration, combat, archery,
  water, mill, sawmill. This is where most of the work lives.
- `crates/dovah-esm/` — the .esm plugin parser (records, cells, NPCs, quests,
  dialogue, water types) plus game-data probes.
- `crates/dovah-nif/` — the .nif mesh parser (nodes, shapes, skinning, Havok
  collision, controllers) plus DDS texture handling helpers.
- `crates/dovah-bsa/` — the .bsa archive reader (LZ4 frame format).
- `crates/dovah-dds/` — the .dds texture decoder (BC1/BC3/BC5/BC7).
- `crates/dovah-papyrus/` / `crates/dovah-pex/` — the Papyrus script
  (.psc/.pex) parser and VM pieces.
- `crates/dovah-strings/` — the .strings localisation reader.
- `crates/ruffle-harness/` — the Ruffle (Flash) test harness and the
  `synthesize_fontlib` code used to run Skyrim's real .swf/.gfx interface.
- `docs/` — reference docs: format notes, the Mudcrab study, the Flash/Ruffle
  feasibility study, and the v0.4.8/v0.4.9 crash diagnoses.
- `vendor/ruffle/*.patch` + `README-DOVAHENGINE.md` — our 7 patches against
  vendored Ruffle 0.7.1 (the Ruffle checkout itself is NOT in this zip).
- `*.sh`, `*.py` (repo root) — render/test/diff helper scripts.

## What is NOT in this zip

- The vendored Ruffle 0.7.1 checkout and the vendored bevy_pbr 0.20.1 crate
  (upstream code; only our Ruffle patches are included).
- Any Bethesda game data (.esm/.bsa/.nif/.dds files) — the engine reads those
  from the player's own copy at runtime.
- Build output (`target/`), test data, screenshots, gameplay recordings,
  and the Mudcrab reference clone (`references/`).
- `AGENTS.md` (the assistant's own operating notes, not source).

## State as of v0.4.13 (2026-11-17, phone playtest)

- Launches; the exterior loads (no crash). Inn is too bright on the phone
  (ambient 100 overshot — desktop calibration does not match the phone).
- Runs 7–16 fps on the phone; the Ruffle HUD's CPU readback ("capture") is
  the biggest single cost, spiking 37–49 ms early in a scene.
- Melee: no hit ever landed in two logged runs (combatants build fine).
  Combat work is currently halted by the owner's order.
- Known rendering defects under fix: grey/washed exterior, white-ghost pine
  trees, rainbow fire-pit artifact, white untextured forge props, a green
  missing-texture box at the workbench, dark featureless NPC faces, a door
  that glows and doesn't fill its frame, a white disc floating indoors.
- Known interaction defects under fix: crosshair weight label reads
  "SWEIGHT", NPC names double-drawn, dialogue lines overlap, topic list runs
  under the touch buttons, PC keyboard prompts on touch, touch buttons draw
  over menus, journal/inventory clipping, dialogue range too generous,
  no interaction prompts, Sven's house exit door opens but never transitions.
- The startup log is gaining press logging (every button press + outcome)
  and light-state logging (ambient, exposure, each active light).
