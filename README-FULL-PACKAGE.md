# Dovahengine — Full Source Package (with vendored code)

Snapshot built 2026-11-17 evening, after v0.4.13. Same engine as
README-SOURCE.md, plus the vendored code that the v0.4.13 zip left out.

## Contents

- `crates/` — all 9 Rust crates (dovah-app, dovah-bsa, dovah-dds,
  dovah-esm, dovah-nif, dovah-papyrus, dovah-pex, dovah-strings,
  ruffle-harness), including `crates/dovah-app/android_res/` (launcher
  icons) and `crates/dovah-app/assets/` (touch UI art).
- `Cargo.toml`, `Cargo.lock` — workspace root.
- `vendor/ruffle/` — the FULL vendored Ruffle 0.7.1 checkout (25,924 files),
  with our 7 patches (`0001`–`0007`) and `README-DOVAHENGINE.md` at its root.
  The patches are applied to this checkout.
- `vendor/bevy_pbr/` — the FULL vendored bevy_pbr 0.20.1 crate (153 files).
  One function changed vs upstream: `getDistanceAttenuation` in
  `src/render/pbr_lighting.wesl` (Skyrim linear falloff instead of Bevy's
  fixed inverse-square). See the `[patch.crates-io]` section in the root
  `Cargo.toml`.

## NOT in this package

- Build output (`target/` — the Ruffle checkout's alone is ~4 GB).
- Any Bethesda game data (.esm/.bsa/.nif/.dds) — the engine reads those
  from the player's own copy of Skyrim SE at runtime.
- Test data, screenshots, gameplay recordings, logs.

## State

See README-SOURCE.md for the v0.4.13 phone-playtest state. Note: this
snapshot includes work-in-progress rendering and interaction fixes landed
after the v0.4.13 zip (touch, dialogue, inventory, scene, character,
pause, combat, ruffle modules) — those fixes are not yet in any APK.
