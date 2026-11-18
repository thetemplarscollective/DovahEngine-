# Milestone 4 gap analysis — the original Skyrim main menu and loading screens

Date: 2026-11-17. Status: diagnosis only (read-only research; no code changed).
Scope: what it takes to build the ORIGINAL main menu (`startmenu.swf`) and
loading screens (`loadingmenu.swf`) via Ruffle, in the engine's current state.
Companion doc: `docs/m5-original-menus-diagnosis.md` (the ten-menu gap
analysis; its shared blockers B1–B11 are cited here by number, not repeated).

Sources read for this doc: `docs/m5-original-menus-diagnosis.md`,
`docs/chatgpt-rendering-presentation-brief-20261117.md` (Phases 5–6),
`crates/dovah-app/src/lib.rs` (AppState, boot flow, loading screen,
door transitions), `crates/dovah-app/src/scene.rs` (`SpawnProgress`),
`crates/dovah-app/src/save.rs` (`list_saves`, `SaveInfo`),
`crates/dovah-app/src/dialogue.rs` (`GameStrings`, ILSTRINGS resolution),
`crates/ruffle-harness/src/main.rs` (modes),
`docs/loading-performance-diagnosis.md` (phase timings),
`interface-extract/flash-exports/scripts/startmenu/scripts/__Packages/StartMenu.as`
(full, read in full),
`interface-extract/flash-exports/scripts/loadingmenu/scripts/__Packages/LoadingMenu.as`
(full), the raw `LSCR` records in `test-data/Skyrim.esm` (byte dump),
and `test-data/strings/Data/Interface/Translate_English.txt` (592 lines,
UTF-16LE, decoded).

## Where things stand

- **There is no main menu.** The app boots into `AppState::Loading`, shows
  the Bevy loading screen (DovahEngine logo + title + subtext + progress
  line), parses the ESM on a worker thread, then goes `Loading → Spawning
  → InGame` straight into the Sleeping Giant Inn (or the `--cell` /
  `--exterior` target). `AppState` is three states only (`lib.rs:153`):
  `Loading`, `Spawning`, `InGame`. There is no `MainMenu` state.
- **The loading screen is Bevy UI and it works.** It stays alive through
  `Loading` AND `Spawning` (`poll_load` + `despawn_loading_screen`,
  `lib.rs:1325-1404`), shows the chunked build's counts
  (`update_loading_progress` → `SpawnProgress::progress_text()`), and is
  respawned on every door transition with the destination's name
  (`door_transition`, `lib.rs:1736`: `"Loading {}..."`) and on save loads
  (`save.rs:715`). The v0.4.13 video shows it running ~60–75 s on the
  exterior load. The GPT brief's Phase 6 says to evolve it, not replace
  it — the Bevy screen is the fallback the Ruffle loading menu must beat.
- **The SWF files exist and are small.** `startmenu.swf` 66,663 B,
  `loadingmenu.swf` 6,160 B, `creditsmenu.swf` 149,015 B,
  `loadwaitspinner.swf` 4,438 B, `credits.txt` (the credits text) — all
  in `interface-extract/interface/`. The loading menu's 6 KB size is the
  first clue: the SWF is text + a level meter only. Everything visual —
  the background, the rotating 3D model, the spinner — is host-rendered.
- **The harness can load these SWFs but cannot drive them yet.**
  `ruffle-harness` takes any SWF path as argv[1], but its modes
  (`demo`/`probe`/`loc`/`jump`/`force`/`upd`/`inventory`) are HUD- and
  inventory-specific drives. A new harness mode per screen (a canned
  `RequestLoadingText` answer, a canned `setupMainMenu` call) is prep
  work, following the M3 pattern: prove in the harness before engine
  integration.
- **No `flash-exports` exist for `creditsmenu.swf` or
  `loadwaitspinner.swf`.** The ffdec export step that produced the other
  menus' `.as` files was never run for these two. The credits menu's
  bridge is therefore unread — a prep step, not a blocker (see §1.6).

## 1. Main menu — `startmenu.swf`

### 1.1 What the SWF actually is (read from `StartMenu.as`, in full)

The menu is a state machine with ten states: `PressStart`, `Main`,
`MainConfirm`, `CharacterLoad`, `CharacterSelection`, `SaveLoad`,
`SaveLoadConfirm`, `DeleteSaveConfirm`, `DLC`, `MarketplaceConfirm`.
The main list is built by `setupMainMenu(...)`, which the host calls
with seven positional arguments:

| Arg | Meaning | Engine value |
|-----|---------|--------------|
| `[0]` | Show QUIT | `true` |
| `[1]` | Saves exist (enables CONTINUE + LOAD) | from `list_saves()` — non-empty |
| `[2]` | Version string | the engine version (`"0.4.14"` etc.) |
| `[3]` | Start in PressStart state | `false` (the console press-start flow) |
| `[4]` | Show DLC item | `false` (no DLC concept beyond the 5 always-loaded plugins) |
| `[5]` | Show HELP item | `false` (no help menu) |
| `[6]` | Show MOD MANAGER item | `false` (no mod manager) |

**Correction to the GPT brief (Phase 5): there is no Settings item in
the start menu.** The brief lists "New Game, Continue, Load Game,
Settings, Credits, Quit" — vanilla puts settings in the pause menu's
System tab (already Bevy-covered, wired to `settings.json`; its Ruffle
version is the M5 pause cutover). The start menu's items are Continue,
New, Load, DLC, Mod Manager, Credits, Quit, Help. Settings is out of
scope for this screen.

The SWF→game calls, per item:

- **CONTINUE** → `CONTINUE` → host answers `ConfirmContinue` (shows the
  confirm screen) → accept calls `ContinueLastSavedGame`.
- **NEW** → `NEW` → host answers `ConfirmNewGame` → accept calls
  `StartNewGame`. See §1.2 — this is the open decision.
- **LOAD** → `PopulateCharacterList([entryList, batchSize])` → character
  selection → save list → `LoadGame([index])`. See §1.3–1.4.
- **QUIT** → confirm screen (`"$Quit to desktop?  Any unsaved progress
  will be lost."`) → `QuitToDesktop`. The host answers with `AppExit`.
- **CREDITS** → `OpenCreditsMenu`. See §1.6.
- **DLC / MOD / HELP** — hidden by the `setupMainMenu` args above; the
  host never sees these calls. (`DoLoadDLCPlugins` / `LoadDLC` need no
  stub if the item is never shown.)
- **`PlaySound` everywhere** — every press, focus move, and confirm
  calls `PlaySound(["UIMenuOK"])` etc. There is no audio system; all
  `PlaySound` calls are no-ops (the stub provider logs them). No work.
- **Platform.** `SetPlatform(0)` = `PLATFORM_PC_KBMOUSE`: mouse buttons
  visible, gamepad buttons hidden, the delete button becomes a mouse
  button with click handling. The SWF already has a mouse mode — taps
  map to mouse clicks through B7, and no gamepad path is needed.
- **Gamer tag / icon.** `ShowGamerTagAndIcon(strGamerTag)` loads
  `img://BGSUserIcon` (B4) — but only when the tag is non-empty. Pass
  `""` and the SWF hides the whole widget. **B4 is avoidable for the
  start menu** (see §3).

### 1.2 NEW — the open decision (not re-litigated, but the handler must exist)

New Game stays UNWIRED (Dylan's 2026-11-16 decision, re-affirmed
2026-11-17). But the SWF's flow requires a host answer either way:
`setupMainMenu` always pushes NEW with `disabled:false` — the host
cannot grey it out — and pressing it calls `NEW`, after which the SWF
waits for the host to invoke the `ConfirmNewGame` callback. The
options, smallest first:

1. **Confirm-and-decline (recommended).** The host's `NEW` handler
   invokes `ConfirmNewGame` with a message ("New Game is not available
   in this build" — the host supplies the confirm text), and the
   `StartNewGame` handler fades the menu back in (`FadeInMenu`) and
   does nothing. Two handlers, no new SWF, honest.
2. **Ignore the press.** The host's `NEW` handler does nothing. The
   SWF waits for a `ConfirmNewGame` that never comes; the menu sits
   there, apparently dead. Rejected — it looks like a bug.
3. **Message box.** Answer via `messagebox.swf` (exists in the
   interface inventory) with a "not available" note. The most
   vanilla-faithful, but it pulls a second SWF + its bridge into this
   milestone for one message. Only if the parent wants it.

The decision is Dylan's; option 1 is the default if he doesn't pick.

### 1.3 CONTINUE — the latest save

`ContinueLastSavedGame` loads the newest save. The engine has this:
`list_saves(dir)` (`save.rs:308`) returns `SaveInfo` newest-first
(location, level, game date, timestamps), and the autosave
(`save-autosave.json`, overwritten on every door transition) is in the
list, so "newest save" is `list_saves(...).first()`. The load itself
reuses the existing machinery: `save_target(data)` → `SceneTarget` →
the worker-thread `load_scene` path (`lib.rs:1730` pattern) → the
loading screen → `Spawning` → `InGame`, with `apply_pending_restore`
re-asserting position/yaw/pitch. No new load path is needed — the
start menu's CONTINUE is a caller of the save system's load, not a
second implementation.

One wrinkle: the autosave is written at the doorway *before* the
transition, so Continue drops the player at the door they just walked
through — correct vanilla behavior (the autosave is the pre-travel
state), and the 30-frame grace period prevents an instant re-fire.

### 1.4 LOAD — the save list, shared with the pause menu

The LOAD flow is the `SaveLoadPanel` pattern, shared with the pause
menu (M5 §2): `PopulateCharacterList` → character selection → the
save list for that character → `LoadGame([index])` → confirm →
load. The pause cutover (M5 #2) builds the `SaveLoadPanel` bridge
first; the start menu reuses it. What the start menu adds:

- **The character list.** Vanilla groups saves by character; the SWF
  asks for the character list before the save list. The engine has no
  character concept (one player, no character creation, no character
  name in `SaveData`). The host's `PopulateCharacterList` answer is a
  one-entry list. What name it carries is a small open question — the
  saves have no name to show (see §5).
- **Save screenshots (B9).** `PrepSaveGameScreenshot` → host answers
  `ScreenshotReady`, and the SWF loads
  `img://BGSSaveLoadHeader_Screenshot` (B4). The save system stores
  JSON only — no screenshots exist. B9 (capture at save time + a
  sidecar or a `SaveData` v3 field) lands with the pause menu; the
  start menu inherits it. Without B9 the load list shows entries with
  no screenshots — the SWF tolerates a missing image (the loader just
  shows nothing), so this degrades gracefully rather than breaking.
- **Delete.** `DeleteSelectedSave` + `DoDeleteSaveUISanityCheck` → the
  host deletes the save file and answers `OnDeleteSaveUISanityCheck`
  (whether a recent save / any loadable save remains), which the SWF
  uses to drop CONTINUE / disable LOAD. Deleting a file is trivial;
  the sanity-check answer is one `list_saves()` call.

### 1.5 The `AppState::MainMenu` design

The brief's Phase 5 is right: the main menu is its own application
state, not an overlay. What changes and what stays:

**What changes:**
- `AppState` gains `MainMenu`, and it becomes the `#[default]` state
  (replacing `Loading`). `build_windowed_app` starts in `MainMenu`
  instead of spawning the loading screen in `Startup`.
- The first load is **deferred**: today the ESM-parse worker thread
  spawns at app start (`lib.rs:881`) and `poll_load` flips
  `Loading → Spawning` when the data arrives. With a menu, the player
  chooses first — CONTINUE/LOAD pick the target, and *then* the load
  starts. The `LoadReceiver` / `poll_load` / `Loading → Spawning →
  InGame` chain is unchanged; only the trigger moves from "app start"
  to "menu choice".
- The menu's Ruffle player (B1) is built at boot and lives for the
  whole menu session — its build cost (shader compile + fontlib
  import, seconds on the phone) lands on startup, not on menu open.
  That cost is currently paid once for the HUD player on the first
  InGame frame; the menu player moves a second player build earlier.
  Whether the menu player and the HUD player share one fontlib build
  (B5) is a boot-order decision: whoever builds first builds the
  fontlib, the other reuses it.

**What stays:**
- The entire door-transition machinery (`door_transition`,
  `load_scene`, the worker thread, `LoadReceiver`, the
  `Loading → Spawning → InGame` chain) is untouched. The menu only
  changes how the FIRST load is triggered.
- The Bevy loading screen stays as the load screen behind the menu's
  choices until the Ruffle loading menu (§2) lands — and stays as its
  fallback afterwards.
- The screenshot path (`--screenshot`, `--load-test`, the seed flags)
  bypasses the menu entirely: it calls `load_first` synchronously
  (`lib.rs:635`) and never enters `MainMenu`. The test suite's
  full-app tests (`windowed_app_builds_and_runs_two_updates`,
  `chunked_build_transitions_*`) are the launch gate — they must
  still pass with the new default state, which means they need a
  "skip the menu" path (the same flags the screenshot path uses, or
  a test-only default). This is a test-design point for the build
  phase, not a blocker.

**The parse gate (the sequencing problem).** The ESM parse (~10 s on
the phone) currently happens during the first load. With a menu, the
player can press CONTINUE before the parse finishes — but loading a
save needs the parsed plugin (the save's cell must resolve). Two
shapes, both workable:
- (a) **Parse in the background while the menu shows.** The worker
  thread starts at boot as today; the menu shows immediately; if the
  player picks CONTINUE/LOAD before the parse lands, the menu waits
  (the SWF's `StartLoadingDLC` / `DoneLoadingDLC` "Loading extra
  content" state is the natural vehicle for the wait — the host
  controls when `DoneLoadingDLC` fires). The menu is usable from
  second one; the parse hides behind it.
- (b) **Parse before the menu.** The app shows the Bevy loading
  screen until the parse completes, then opens the menu. Simpler,
  but the player waits ~10 s staring at a loading screen before
  seeing the menu — the thing the menu exists to replace.
Recommend (a): the menu's whole point is to be the first thing the
player sees, and the parse is already a worker thread — it just
needs to start at boot instead of at first-load.

### 1.6 CREDITS — `creditsmenu.swf`

`OpenCreditsMenu` fades the start menu out and the host loads
`creditsmenu.swf` (149,015 B). The credits content is `credits.txt`
(exists in `interface-extract/interface/` — opens with the Skyrim
logo + "THE ELDER SCROLLS V: SKYRIM SPECIAL EDITION"). Two prep
items: (1) no ffdec export exists for `creditsmenu.swf`, so its
bridge is unread — run the export before implementation; (2) the
SWF must be served `credits.txt` — the navigator backend needs a
file-serving path for it (the `MemoryNavigatorBackend` serves the
fontlib from memory today; `credits.txt` needs the same treatment or
a file URL). Small gaps, both contained.

## 2. Loading screens — `loadingmenu.swf`

### 2.1 What the SWF actually is (read from `LoadingMenu.as`, in full)

The entire bridge is two `respond` calls, both fired from
`FadeInMenu()`:

- `RequestLoadingText([])` → host answers `SetLoadingText(astrText)`.
- `RequestPlayerInfo([])` → host answers
  `SetLevelProgress(afCurrentLevel, afLevelProgress, abShowMeter)`.

There are no game→SWF callbacks, no input handling, no progress bar,
no destination field, no model hooks. The SWF draws the lore text
(bottom-right, fading in) and the level meter (top-right: level
number + a `Components.Meter` bar). **Everything else the player
sees on a vanilla loading screen — the background image, the
rotating 3D model, the spinner — is rendered by the host and
composited with the SWF frame.** The 6,160 B file size says the same
thing.

### 2.2 The lore text source — IDENTIFIED: LSCR records, DESC via ILSTRINGS

This closes M5 open question 7. The lore/tip text lives in **LSCR
(Load Screen) records** in the ESMs, and the field is **DESC — a u32
localized-string ID resolved through `.ILSTRINGS`**.

Evidence (read from the raw bytes of `test-data/Skyrim.esm`, offsets
243,892,596+ — the LSCR group sits near the end of the file):

```
Record 1: form 0x00000e8a, EDID "ChargenLoadScreen"
  DESC 04 00 c1 26 01 00     -> string ID 0x000126c1
  CTDA 20 00 ...             -> 1 condition (32 bytes)
  NNAM 04 00 63 a9 10 00     -> form ID 0x0010a963
  SNAM 04 00 cd cc 4c 3e     -> f32 0.2
  RNAM 06 00 00 00 00 00 78 00
  ONAM 04 00 4c ff b4 00     -> form ID 0x00b4ff4c
  XNAM 0c 00 ...             -> 3 x f32 (-25.0, 0.0, -115.0)
  MOD2 24 00 "Cameras\LSCameraPanRGTZoomINBig.nif"
Record 2: form 0x0010e732, EDID "xCityRiften03b"
  DESC 04 00 ba 23 01 00     -> string ID 0x000123ba
  CTDA x2, NNAM 0x0010cc2e, SNAM f32 1.6, ONAM 0x00b4ff4c (same object),
  XNAM (-10.0, 0.0, -10.0), MOD2 "Cameras\LS..."
```

Counts ("LSCR" byte occurrences, each record contributing one type
tag plus one group label per file): Skyrim.esm 47, Dawnguard.esm 7,
Dragonborn.esm 2, HearthFires.esm 1, Update.esm 1 — **on the order
of 50 load screens** across the five plugins.

The LSCR field layout, as proven by the dump:

| Field | Size | Meaning (proven / inferred) |
|-------|------|------------------------------|
| EDID | string | Editor ID (`ChargenLoadScreen`, `xCityRiften03b`) |
| DESC | u32 | **The lore text — a localized string ID.** Resolves in `.ILSTRINGS`. Proven: the value is a 4-byte ID, not text. |
| CTDA | 32 B each | Conditions selecting when this screen shows (destination-specific selection). |
| NNAM | u32 form ID | Background texture reference (record type unverified — resolve 0x0010a963's type during implementation). |
| SNAM | f32 | Unknown scalar (0.2 / 1.6 in the samples). Needs the CK reference to pin down. |
| RNAM | 6 B | Unknown (3 x i16 in the samples). Needs the CK reference. |
| ONAM | u32 form ID | **The displayed object — the rotating 3D model.** Same form ID (0x00b4ff4c) in both samples. |
| XNAM | 3 x f32 | Position triple for the model/camera. |
| MOD2 | string | **Camera animation NIF** (`Cameras\LSCameraPanRGTZoomINBig.nif`) — the pan/zoom. |

What the engine needs for the text: a small `lscr.rs` module in
`dovah-esm` (the `water.rs` / `quest.rs` pattern) that walks the
`LSCR` group and extracts EDID / DESC / CTDA / NNAM / ONAM / XNAM /
MOD2 per record. **No new string machinery is needed**: `GameStrings`
already resolves u32 IDs in each plugin's `.ILSTRINGS`
(`dialogue.rs:204-263` — the same path dialogue text takes). DESC
resolves through the record's own plugin set, exactly like `NAM1`.
The parser side is a day's work, not a research project.

### 2.3 Destination-specific selection — the CTDA gap

The CTDA conditions are how vanilla picks a screen for the
destination (a Riften screen shows when loading Riften). The engine
has **no condition evaluation** — dialogue shows all topics for the
same reason, documented not faked. Full CTDA evaluation is a
Papyrus-adjacent system (function calls, aliases, global values) and
is out of scope for this milestone. The pragmatic options:

1. **Ignore conditions initially (recommended for v1).** Pick a
   random LSCR record per load. The text is real lore, the model is
   real — only the destination match is missing. Honest and shippable.
2. **Location-condition subset.** Evaluate only the conditions that
   test location/cell (a small subset of CTDA function IDs), ignore
   the rest. Destination matching for the common case, without the
   full condition system. The follow-up if (1) feels wrong in play.
3. **Full CTDA evaluation.** A separate milestone (it also unblocks
   dialogue conditions). Not this one.

### 2.4 The rotating 3D model — the biggest unknown

This is the largest single gap in Milestone 4, and the hardest part
of the loading screen. What vanilla does: renders the ONAM object's
NIF model rotating on a background, with the MOD2 camera NIF driving
a slow pan/zoom. What the engine must build:

1. **Resolve ONAM → a renderable model.** ONAM is a form ID
   (0x00b4ff4c in both samples). Resolve it through the plugin:
   what record type is it, and what is its MODL? (An ARMO? A WEAP?
   A STAT? The samples share one ONAM, which suggests a small set
   of display objects — but that is two records out of ~50; the
   full ONAM set must be enumerated during implementation.)
2. **Render the model to a texture** (or a 3D viewport behind the
   Ruffle frame) and composite it with the SWF frame. This is the
   B8 (`UpdateItem3D`) compositing problem at full screen instead
   of a 740×410 card — the same render-to-texture + composite shape
   the inventory needs, which is why the loading screen de-risks
   B8 for the M5 menus (see §4).
3. **Rotate it.** The model turns slowly; the rotation is host-side
   (the SWF has no model hooks at all).
4. **The MOD2 camera NIF.** `Cameras\LSCameraPanRGTZoomINBig.nif`
   is a camera path animation. Playing it needs the camera
   animation from the NIF (NiControllerSequence / camera keys) —
   the sequence player queued for the sawmill log cycle, applied
   to a camera. **Pragmatic first step: a static camera + a
   rotating model.** The MOD2 playback is the follow-up; a static
   camera still reads as a loading screen, a missing model does not.
5. **The background.** NNAM is the background texture reference
   (form ID; record type unverified). Until it resolves, the
   fallback is the current loading screen's dark background
   (0.05, 0.05, 0.07) behind the model + SWF frame.
6. **Performance during the build.** The model render + the Ruffle
   step + the chunked scene build share the frame. The build owns
   an 8 ms/frame budget; the loading menu's render must fit in
   what's left without starving the build or the movie step. This
   needs measuring on the phone, not assuming — the same caveat
   M5 §9 puts on the movie step during `Spawning`.

### 2.5 Real progress — the SWF has nowhere to put it

The GPT brief's Phase 6 asks for "real loading progress". The
honest finding: **the SWF has no progress bar.** `SetLevelProgress`
is the *player's level* meter (level number + skill-progress bar),
not load progress. Vanilla shows no load percentage — the
"still loading" signal is `loadwaitspinner.swf` (4,438 B, a
separate tiny SWF) plus the fading text.

The engine, meanwhile, *has* real progress: `SpawnProgress::progress()`
(`scene.rs:773`) is a monotonic 0.0–1.0 fraction (Objects 82%,
Dressing 8%, MapBake 9%, Finish 1%), and `progress_text()` gives the
honest counts ("Spawning objects 123/437"). The measured phase
split (loading-performance diagnosis, v0.4.13 phone logs): the
Objects phase is 80–90 s of a ~90 s exterior load; the worker
`load_scene` ~2 s; Finish 0.01 s. The options for showing it:

1. **Keep the Bevy progress line as an overlay (recommended).**
   The Ruffle frame composites over the 3D model; the Bevy
   progress text draws on top (or below the SWF text). The player
   keeps the honest counts the current screen already shows. No
   new SWF, no second player.
2. **Run `loadwaitspinner.swf` alongside.** Vanilla-faithful, but
   one player = one movie (B1) — the spinner needs its own player
   (a second capture stall, B10) or a Bevy-drawn spinner. More
   machinery for less information than option 1.
3. **Drop the progress display.** Accept vanilla's no-progress
   behavior. A regression in information — the current screen shows
   counts, and a 90 s load with no progress reads as a hang. Not
   recommended until Milestone 3 cuts load times.

### 2.6 The level meter — hide it until progression exists

`SetLevelProgress(afCurrentLevel, afLevelProgress, abShowMeter)`:
the player level is static (1) and skill progression is not
tracked, so the honest answer is `(1, 0.0, false)` — hide the meter
(`abShowMeter = false`) until the progression system exists. Showing
an empty meter at level 1 is also honest but reads as broken; hiding
it is the cleaner call. Revisit when skills gain XP.

### 2.7 Destination name — the SWF has no field for it

The current Bevy screen titles with the destination ("Loading
Riverwood..."). The Ruffle loading menu has no destination field —
`SetLoadingText` is the only text. Options: (a) prefix the lore
text with the destination ("Riverwood — <lore>"); (b) keep a Bevy
title overlay (same overlay as §2.5 option 1 — one overlay carries
both); (c) drop it. Recommend (b): the overlay already exists for
progress, and the destination name costs one more line on it.

### 2.8 First-load sequencing — the parse chicken-and-egg

On the FIRST load, the loading menu cannot show lore text until the
ESM parse finishes — the LSCR data comes from the parsed plugin,
which doesn't exist until the parse lands (~10 s on the phone). The
shape that follows:

- **During the parse:** the Bevy loading screen (as today — "Parsing
  Skyrim.esm..."). The Ruffle loading menu has no data yet.
- **During `Spawning`:** the Ruffle loading menu, with the LSCR text
  + model + the Bevy progress overlay.
- **Door transitions:** no problem at all — the plugin is parsed,
  the LSCR data is in memory, the Ruffle menu shows from the first
  frame of the transition.

The cut from the Bevy screen to the Ruffle menu mid-load is a
visible transition; the menu's `FadeInMenu` (which fires the two
`respond` calls) is the natural seam — call it when `Spawning`
starts.

## 3. What the M5 shared blockers (B1–B11) mean for these two screens

| Blocker | Main menu | Loading screen |
|---------|-----------|----------------|
| **B1 — one player per movie** | Own player, built at boot, lives for the menu session. Build cost lands on startup. | Own player, built at load start. **Cache it for the whole session** — the menu is used on every door transition; rebuilding per transition would pay the shader-compile + fontlib cost every time. |
| **B2 — `$` tokens** | **Blocks it outright.** Every menu item is a `$` token (`$CONTINUE`, `$NEW`, `$LOAD`, `$CREDITS`, `$QUIT`, `$DOWNLOADABLE CONTENT`, `$MOD MANAGER`, `$HELP`, `$Load this game?`, `$Delete this save?`, `$Quit to desktop?...`). All verified present in `Translate_English.txt` (592 `$TOKEN\tTranslation` pairs) — the data exists; the resolution mechanism (B2) is the gap. | Barely touches it. The lore text comes from LSCR DESC via ILSTRINGS, not `$` tokens; the SWF's own static text is minimal (the level label is a number). |
| **B3 — tag-1009 keycaps** | Applies to the save-list buttons' key hints. The touch question (what shows where vanilla shows `[E]`?) is the same design decision as M5. | **Does not apply.** No input, no key hints. |
| **B4 — `img://` provider** | **Avoidable for the menu itself** (pass `""` as the gamer tag and the SWF hides the icon widget). **Required for the load list** (save screenshots, via `SaveLoadPanel` — shared with the pause menu). | **Does not apply.** No `img://` loads. |
| **B5 — fontlib, built once** | The menu player is the FIRST player built at boot — it builds the fontlib (or the HUD player does, if the menu is skipped). Boot-order decision. | Shares the fontlib with the HUD + menu players. |
| **B6 — `safeRect` values** | Applies — the menu is the first thing the player sees; the list and version text must clear the cutout. | Applies — the lore text sits bottom-right, the level meter top-right; both must clear the gesture bar / cutout. |
| **B7 — touch → Ruffle** | Applies — the SWF's mouse mode (`SetPlatform(0)`) takes taps as clicks; list navigation needs `NavigationCode` keys. | **Does not apply.** No input. |
| **B8 — `UpdateItem3D` composite** | Does not apply (no 3D model in the start menu). | **Applies, at full screen** — the rotating model IS the B8 problem, plus the MOD2 camera NIF. The loading screen is where B8 is hardest, and where solving it pays off for inventory/container/magic. |
| **B9 — save screenshots** | Applies (the load list). Lands with the pause menu; the start menu inherits it. Degrades gracefully without it (entries with no image). | Does not apply. |
| **B10 — capture stall** | HUD player is not running at boot, so no double stall. But the menu player stepping at 30 fps competes with the ESM-parse worker for CPU — measure, don't assume. | **Cheapest menu on perf.** HUD player is not running during load, so the menu player is the only Ruffle cost. The open question is the movie step + model render vs the 8 ms build budget. |
| **B11 — quest data shapes** | Does not apply. | Does not apply. |

Net: the main menu is blocked by B1, B2, B5, B6, B7 (+ B9/B4 for the
load list); the loading screen is blocked by B1, B5, B6, B8 — and
by the LSCR work (§2.2–2.4), which is Milestone 4's own new work,
not a shared blocker.

## 4. Build order, ranked

| # | Screen | Why this position |
|---|--------|-------------------|
| 1 | **Loading screens** | Smallest bridge in the whole Ruffle program (two `respond` calls, no input, no save list, no AppState change). Highest frequency — exercised on every door transition. Its 3D-model composite is the biggest unknown, and solving it de-risks B8 for inventory/container/magic (M5 #3–5). Its only new engine work is the `lscr.rs` parser (small) + the model composite (large, but shared with B8). |
| 2 | **Main menu** | Needs the `AppState::MainMenu` + boot-flow change (the riskiest edit in this milestone — it touches every startup). Its save-list flow reuses the pause menu's `SaveLoadPanel` bridge (M5 #2), which should land first. Its NEW-handler decision (§1.2) is Dylan's call and can be made while the loading screen builds. |

**Loading screens first, main menu second.** The M5 report ranked
them 9th and 10th in the same order, for the same reasons. The
loading screen is the screen the player sees most and the screen
whose hardest problem (the 3D composite) unblocks the most other
work; the main menu is the screen whose dependencies (pause
`SaveLoadPanel`, the AppState change, the NEW decision) land better
after.

## 5. Open questions for the parent

1. **NEW handler (§1.2):** confirm-and-decline (recommended), ignore,
   or message box? The item is present but unwired either way.
2. **Character list (§1.4):** the LOAD flow asks for a character
   list; the engine has one player and no character name. What name
   shows for the single entry?
3. **Loading progress (§2.5):** keep the Bevy progress line as an
   overlay (recommended), run the spinner SWF, or drop progress?
4. **Destination name (§2.7):** keep it on the Bevy overlay with the
   progress (recommended), prefix the lore text, or drop it?
5. **Level meter (§2.6):** hide it until skill progression exists
   (recommended), or show level 1 at 0%?
6. **DLC / MOD / HELP items:** hidden (pass `false` in
   `setupMainMenu`) — confirm. No DLC list, no mod manager, no help
   menu in this milestone.
7. **LSCR selection (§2.3):** random screen per load (recommended
   for v1), or the location-condition subset?
8. **First-load sequencing (§1.5, §2.8):** parse in the background
   while the menu shows (recommended), or parse before the menu?
9. **Menu-open latency budget (M5 Q5, applies here too):** the menu
   player's build cost lands on startup — is a slower boot
   acceptable, or does the player build need to overlap the ESM
   parse?

## 6. What to build first (the shared work, before screen 1)

1. **B2 — `$` token resolution.** Blocks the main menu outright.
   Decide the approach (Ruffle patch vs load-time rewrite) before
   screen 2; screen 1 barely needs it.
2. **`lscr.rs` in `dovah-esm`.** Walk the LSCR group; extract EDID /
   DESC / CTDA / NNAM / ONAM / XNAM / MOD2. Resolve DESC through the
   existing `GameStrings` ILSTRINGS path. Enumerate the full ONAM
   set and resolve NNAM's record type (0x0010a963 first).
3. **B1 — the menu-player slot, with session caching.** One player
   per menu; the loading-menu player is cached for the whole session
   (it is used on every transition); the menu player lives for the
   menu session.
4. **B8 — the 3D composite, proven on the loading screen.** Static
   camera + rotating model first; MOD2 camera playback as the
   follow-up. The same composite serves inventory/container/magic.
5. **B5 — fontlib once, shared.** Whoever builds first (menu player
   at boot) builds it; the rest reuse it.
6. **B6 — safeRect values.** Read the Android insets via the
   existing JNI plumbing (`android.rs`).
7. **Harness modes for both screens.** A canned `RequestLoadingText`
   / `RequestPlayerInfo` drive for `loadingmenu.swf`, and a canned
   `setupMainMenu` + `NEW`/`CONTINUE` drive for `startmenu.swf`.
   Prove both in the harness before engine integration.
8. **ffdec exports for `creditsmenu.swf` and `loadwaitspinner.swf`.**
   The credits bridge is unread until the export runs.
