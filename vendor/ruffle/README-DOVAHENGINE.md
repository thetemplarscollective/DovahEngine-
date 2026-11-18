# Vendored Ruffle

Ruffle 0.7.1, vendored from the git tag `v0.7.1`
(commit `89a7049965f3090e706265670df309a5128233ac`) for the Dovahengine
Flash-interface work (Ruffle M1, 2026-11-16).

## Why vendored, not a git dependency

Ruffle 0.7.1's **public** API cannot invoke a function defined on a movie's
root timeline. The only host→SWF channel is `Player::call_internal_interface`,
which only reaches callbacks the SWF registered via
`ExternalInterface.addCallback`. Skyrim's menus register their GameDelegate
callbacks *inside* `InitExtensions` — a frame-1 global the host must invoke
first. That is a chicken-and-egg the stock API cannot break: without invoking
`InitExtensions`, the GameDelegate bridge never starts, and the HUD never
responds to `SetLocationName` / `SetHealthMeterPercent` / etc. (proven in M1:
the unmodified HUD rendered fine but ignored every `call_internal_interface`
drive call).

The `avm1` / `avm2` / `display_object` modules are private, so no external
crate can work around this with `mutate_with_update_context` either — the
study's "fiddly-but-feasible" variable-injection claim does not hold for an
external crate.

## The patch

`0001-call-root-function.patch` adds two public methods to `Player`
(`core/src/player.rs`, next to `call_internal_interface`):

```rust
pub fn call_root_function(
    &mut self,
    name: &str,
    args: impl IntoIterator<Item = ExternalValue>,
) -> ExternalValue

pub fn call_clip_function(
    &mut self,
    path: &str, // dotted, e.g. "LocationLockBase.LocationNameBase"
    name: &str,
    args: impl IntoIterator<Item = ExternalValue>,
) -> ExternalValue
```

`call_root_function` resolves `name` on the root clip's AVM1 object and
calls it in a stub activation, mirroring `Callback::call`'s AVM1 branch in
`core/src/external.rs`. `call_clip_function` walks a dotted path through
the AVM1 object tree first (a named child clip reads back as
`Value::MovieClip`, resolved via `coerce_to_object`) — Scaleform's `Invoke`
can target a named clip (`HUDMovieBaseInstance.RegisterComponents`), and
this is that operation.
AVM1 only (Skyrim's SWFs are all AVM1 — zero DoABC tags in the 13 menus
studied); AVM2 movies return Null; a missing function returns Undefined.

`0002-clip-state.patch` (M2, diagnostic) adds one more public method in
the same spot:

```rust
pub fn clip_state(&mut self, path: &str) -> String
```

It walks the same dotted path and reads `_currentframe`, `_totalframes`,
`_x`, `_y`, `_visible`, `_alpha`, and `text` (TextFields) off the AVM1
object, plus `CurrentPercent`, `TargetPercent`, `Empty`, and `Full`
(Meter objects — undefined on clips), formatted as one line — or
`NOT FOUND` (naming the failing segment) if the path does not resolve. The harness cannot read clip
properties through any stock Ruffle API, and a method's return value
cannot distinguish a landed call from a missing path when the method
returns Undefined — this is what made the M1 probe's negative result
uninterpretable until the state dump existed. Diagnostic-only; it is not
part of the engine's runtime path and needs no upstreaming.

`0003-stage-visible-rect.patch` (M2) adds Scaleform's `Stage.visibleRect`
and `Stage.safeRect` getters to the AVM1 Stage object
(`core/src/avm1/globals/stage.rs`), returning `flash.geom.Rectangle`
objects built with the existing `new_rectangle` helper. `visibleRect` is
the full stage rect in stage coordinates; `safeRect` is the same rect
while there is no safe-area inset. Skyrim's HUD anchors itself with
`GlobalFunc.Lock`, which reads both — without them every anchored clip
parked at global (0, 0) and the meters/location rendered off-screen.
This one is a genuine upstream gap for Scaleform-style movies (Ruffle
has no `visibleRect`/`safeRect` at all) and is worth proposing upstream
alongside 0001.

`0004-goto-float-frame.patch` (M2) fixes `gotoAndStop`/`gotoAndPlay`
with non-integer frame numbers (`core/src/avm1/globals/movie_clip.rs`,
`goto_frame`). Stock Ruffle only ran a direct goto `if n.fract() ==
0.0`; a float fell through to the label branch, where the stringified
number fails path resolution and the goto silently does nothing. Flash
truncates float frame numbers (ECMA ToInt32) — Skyrim's HUD
`Meter.Update` passes raw `Lerp` floats to `gotoAndStop` (only
`SetPercent` floors), so under stock Ruffle the bars tweened their
percents correctly but the bar clip never moved off its Full frame.
The patch drops the `fract` guard so every `Value::Number` truncates
via the existing `f64_to_wrapping_i32`; NaN/inf/negatives still no-op
via the `frame > 0` check. Also a genuine upstream divergence, worth
proposing.

`0005-nested-imports.patch` (M3) fixes nested `ImportAssets`
(ruffle-rs/ruffle#22426) — the go/no-go gate for Skyrim's
component-based menus (inventory/magic/container). Three changes:

* `core/src/loader.rs`, `load_asset_movie`: preload the imported
  movie to completion, resuming after each nested import, instead of
  calling `clip.preload` once and declaring the import done. A single
  preload stops at the first `ImportAssets` tag, so a component that
  imports fonts before its own exports (all three inventory
  components do) never registered them, and the importer failed to
  instantiate the component (`Tried to instantiate a non-registered
  character ID 87/89/90`). Each nested import runs as its own
  spawned future and clears the clip's awaiting-import flag when it
  resolves; the loop yields between attempts (`YieldOnce`, a
  hand-rolled yield future — no new dependency, wasm-safe) so the
  executor can run it. A failed import now also calls
  `finish_importing` on the importer (previously a single failed
  component fetch wedged the importer in preload forever and its
  frame 1 never ran). A 10,000-resume cap with a warning guards
  against an import that never resolves.
* `core/src/display_object/movie_clip.rs`, `preload`: only run
  `import_exports_of_importer` once the clip is fully preloaded.
  Copying the importer's exports earlier occupied character IDs the
  clip's own later definitions (or its still-pending imports) need,
  and those definitions then failed to register with a collision —
  the three inventory components lost 65 shapes this way (their own
  IDs 38+ overlap the root's export IDs). The copied exports are
  only ever looked up by name at runtime, which cannot happen before
  preload completes anyway.
* `core/src/library.rs` (`register_character_overwrite`, used by
  `export_assets` for the parent registration): an imported asset
  wins over whatever already sits at its ID. The import reservation
  is authoritative; in a valid SWF the ID cannot also be one of the
  clip's own definitions, so the displaced character is always a
  copied export or a previous registration of the same import. This
  is what lets the components' fonts land (e.g. `$EverywhereMediumFont`
  at ID 37, which the root's `360_RB` export used to occupy).

With 0005, all three inventory components preload to `true`, zero
collisions, zero non-registered IDs, and the inventory menu renders
with real fonts and stub data (see `crates/ruffle-harness/README.md`,
M3). The HUD render is pixel-identical to the M2 proof, so the
shared loader paths did not regress. This is the upstream fix for
#22426 and is worth proposing as a PR.

`0006-set-clip-variable.patch` (M3) adds one more public method in the
same spot as 0001/0002:

```rust
pub fn set_clip_variable(
    &mut self,
    path: &str, // dotted, e.g. "Menu_mc.InventoryLists_mc.ItemsList"
    name: &str, // e.g. "entryList"
    value: ExternalValue,
) -> bool
```

Scaleform's `SetVariable`: the game populates the menus' lists by
setting `entryList` arrays directly on clips, and no GameDelegate
handler exists for it. Same dotted-path walk as `call_clip_function`,
same `into_avm1` conversion (objects and lists cross over). Returns
`false` if the path does not resolve. AVM1 only. Worth proposing
upstream alongside 0001 — it is the same class of host API gap.

`0007-safe-area-insets.patch` (M4) adds a host setter for the
safe-area insets, so the phone build can narrow `Stage.safeRect`
for notches and gesture bars (the HUD anchors its elements to the
safe rect). Three files:

* `core/src/display_object/stage.rs`: a `safe_area_insets:
  Cell<(u32, u32, u32, u32)>` field on `StageData` (left, top,
  right, bottom, in pixels; default `(0, 0, 0, 0)`), plus a
  getter/setter on `Stage`.
* `core/src/avm1/globals/stage.rs`: `safe_rect` returns the stage
  rect minus the insets, clamped so it never inverts. Zero insets
  (desktop) = the full stage rect, exactly the 0003 behaviour.
* `core/src/player.rs`: `Player::set_safe_area_insets(left, top,
  right, bottom)`, mirroring `set_window_mode` (via
  `mutate_with_update_context`).

The engine calls it at player creation with the configured insets
(zero for the desktop proof; the phone's real insets come from the
Android safe-area API later). Worth proposing upstream with 0003 —
it completes the Scaleform `Stage` API the HUD depends on.

## Upstream

This is a minimal, general-purpose addition — the natural upstream fix for
Scaleform-style host-driven movies. It should be proposed to ruffle-rs/ruffle
(an issue + PR) so a future Ruffle release can replace this vendor copy.
Until then, the harness depends on this tree by path (see
`crates/ruffle-harness/Cargo.toml`).

## Reproducing

```sh
rsync -a --exclude=.git \
  ~/.cargo/git/checkouts/ruffle-b1b568dcb5621339/89a7049/ vendor/ruffle/
cd vendor/ruffle && patch -p0 < 0001-call-root-function.patch
cd vendor/ruffle && patch -p1 < 0002-clip-state.patch
cd vendor/ruffle && patch -p1 < 0003-stage-visible-rect.patch
cd vendor/ruffle && patch -p1 < 0004-goto-float-frame.patch
cd vendor/ruffle && patch -p1 < 0005-nested-imports.patch
cd vendor/ruffle && patch -p1 < 0006-set-clip-variable.patch
cd vendor/ruffle && patch -p1 < 0007-safe-area-insets.patch
```

(The patch paths are relative to the repo root; `patch -p0` from
`vendor/ruffle/` applies cleanly against a pristine 0.7.1 tree. 0002–
0007 are git-style `a/`/`b/` paths, so they take `-p1`, applied in
order after 0001. The chain was verified 2026-11-16: applying
0001–0006 to a pristine checkout reproduces the vendored
`loader.rs`, `library.rs`, `display_object/movie_clip.rs`, and
`player.rs` byte-for-byte; 0007 was verified separately against
the pre-0007 vendored files (all three touched files reproduce
byte-for-byte).)
