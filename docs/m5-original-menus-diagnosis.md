# M5 gap analysis — replacing the Bevy menus with the original Scaleform menus via Ruffle

Date: 2026-11-17. Status: diagnosis only (read-only research; no code changed).
Scope: what each original Skyrim `.swf`/`.gfx` menu needs that the harness/engine
does not have yet, ranked by build order, with the shared blockers named.
Not re-investigated: nested `ImportAssets2` — fixed by patch 0005 (M3), upstream
issue ruffle-rs/ruffle #22426. That gate is closed.

Sources read for this doc: `crates/dovah-app/src/ruffle.rs` (full, 2,098 lines),
`crates/ruffle-harness/` (README status doc + `src/provider.rs`, `src/runner.rs`,
`src/fontlib.rs`, `src/navigator.rs`), `docs/flash-interface-feasibility.md`,
`docs/chatgpt-rendering-presentation-brief-20261117.md` (Phase 2), the `.as`
exports under `interface-extract/flash-exports/scripts/` (2,150 files), the
study docs under `interface-extract/study/`, `crates/dovah-app/src/lib.rs`
(AppState/loading screen), `android.rs`, `interact.rs`, `touch.rs`, and the
vendored Ruffle `Player` API (`vendor/ruffle/core/src/player.rs`).

## Where things stand

- **The real HUD works.** Unmodified `hudmenu.gfx` renders live in the engine
  over the 3D scene (M4), driven by real game state. One Ruffle player lives in
  the render world as a `NonSend` resource, built lazily in
  `RenderSystems::PrepareAssets`, sharing Bevy's wgpu device. Compositing is
  CPU readback (`capture_frame()` → mailbox → Bevy `Image` on a fullscreen
  `ImageNode`); GPU compositing is deferred to M6. The movie steps at its own
  30 fps, capped at one step per render frame (`movie_steps_due()`). The
  fontlib is synthesized at runtime from Patch.bsa's `fonts_en.swf`
  (`synthesize_fontlib()`, 16 fonts, Patch wins). The Bevy HUD is kept as
  fallback (`--ruffle-hud` flag + `GameSettings.ruffle_hud`, default true).
- **The harness is the test bench.** `crates/ruffle-harness` runs any menu
  headless with canned data: modes `demo` / `probe` / `loc` / `jump` / `force` /
  `upd` / `inventory`. The `inventory` mode already drives `inventorymenu.swf`
  with canned entry lists; `upd` drives `UpdateItemCardInfo`. Every menu below
  should be proven in the harness before engine integration — that is the M3
  pattern that worked.
- **The Bevy stand-ins** (dialogue, pause, inventory, magic, stats, map) are
  real, working UI. Cutover is per-menu at parity: the Bevy version stays until
  its Ruffle replacement proves itself on the phone, then the Bevy version is
  removed for that screen (M6 cutover). Nothing below removes a Bevy menu
  before its replacement is proven.
- **AppState is three states only** (`lib.rs:153`): `Loading`, `Spawning`,
  `InGame`. There is no `MainMenu` state. Menu-open flags are per-module
  resources: `DialogueOpen`, `InventoryOpen`, `PauseOpen`,
  `CharacterMenuOpen` (pause.rs:238, touch.rs:865-868). There is no
  `FavoritesOpen`, no `ContainerOpen`, no `MapOpen` (the map is a character
  tab). The loading screen is Bevy UI (`LoadingScreen` / `LoadingTitle` /
  `LoadingText` / `LoadingProgress` markers, lib.rs:218-233) with the
  DovahEngine logo and the chunked-build counts.

## The shared blockers (unblock several menus at once)

### B1. Multi-player architecture — no movie swap on Ruffle's Player
The engine has exactly ONE Ruffle player (the HUD). Ruffle's `Player` takes
its movie once at build time (`PlayerBuilder::with_movie`, player.rs:2992);
there is no public `load` / `set_movie` / `swap_movie` method. Each menu
therefore needs its **own `Player` instance** (its own `Harness`), built when
the menu opens — or prebuilt and kept.

Design consequences:
- Menus are modal (one open at a time), so **one menu-player slot** suffices:
  build the player for the menu being opened, drop it on close. But a player
  build costs shader compile + fontlib import (seconds on the phone) — opening
  a menu would hitch unless players are prebuilt/cached. The build cost and
  the caching strategy (one player per menu type, kept alive vs rebuilt per
  open) are the first M5 implementation decision.
- Each player holds its own `TextureTarget` (1280×720 RGBA ≈ 3.7 MB) +
  fontlib + movie data. Memory is modest; the cost is per-player.
- The fontlib must be built **once and shared** across players, not
  re-synthesized per player (see B5).
- The HUD player and the menu player share Bevy's wgpu device (the HUD
  already proves one shared-device player works; a second player on the same
  device is the same pattern, unproven).

### B2. `$` translation tokens
Every menu's static text is `$TOKEN` strings (`$Accept`, `$Damage`,
`$Weight`, …). In vanilla, the game resolves these at display time through
its GFx translator. Ruffle has no equivalent hook — an untranslated `$TOKEN`
renders literally.

- The data is available: `Translate_English.txt` (592 lines, UTF-16LE,
  `$TOKEN\tTranslation` per line) is in the game data
  (`test-data/strings/Data/Interface/Translate_English.txt`, from the BSA).
  The engine must read it from the BSA at runtime via the asset provider
  (note the on-disk case is `Translate_English.txt`; the provider lookup
  must be case-insensitive or normalized).
- Candidate approaches (not decided): (a) patch Ruffle to intercept text
  setting and substitute tokens; (b) pre-process each SWF's string table
  before loading (a load-time rewrite, not a shipped modification — the
  `.swf` files stay unmodified on disk); (c) inject translated strings
  through the bridge wherever the SWF reads them (only works where the
  SWF asks the host for the string — most `$` tokens are static text fields
  the SWF sets itself, so (c) covers only a minority).
- This blocks **every menu** — no menu reads correctly until tokens resolve.

### B3. Tag-1009 keycap art
The keycap glyphs (the `[E]`, `[Tab]`, `[R]` key images the menus draw next
to prompts) live in a Scaleform-specific tag (tag 1009) that Ruffle does not
render (feasibility doc). Two questions, one technical and one design:
- Technical: patch Ruffle to render tag 1009, or replace the keycap art with
  host-drawn equivalents.
- Design: on a phone the PC keycaps are wrong anyway — the current UI fix
  round is removing `[Tab]`/`E`/`R` prompts from the Bevy UI. The Ruffle
  menus' key hints need touch equivalents (a tap target, not a keycap). This
  is a decision for the parent, not a pure engineering gap: **what does a
  Ruffle menu show where vanilla shows `[E]`?**

### B4. The `img://` provider — not implemented anywhere
Zero hits for `img://` in `crates/ruffle-harness/src/` or
`crates/dovah-app/src/`. Three `img://` loads are confirmed in the menu
scripts; a full sweep of all ten menus is implementation work:
- `img://Local_Map` — the map menu's world image
  (`map/scripts/__Packages/Map/LocalMap.as:47`).
- `img://BGSSaveLoadHeader_Screenshot` — the save/load panel's screenshot
  (`quest_journal/scripts/__Packages/SaveLoadPanel.as:244`).
- `img://BGSUserIcon` — the start menu's profile icon.
Ruffle's `NavigatorBackend`/`LoadManager` must be taught to serve `img://`
URLs from host-provided images. This blocks the map, the pause save/load
panel, and the start menu.

### B5. Fontlib production shaping
The synthesized fontlib works in the harness (16 fonts from `fonts_en.swf`,
Patch wins). Production needs:
- The full set of `$` font export names (15 in the feasibility doc) verified
  against every menu, not just the HUD and inventory.
- The fontlib built once and shared across the HUD + menu players (see B1),
  not re-synthesized per player.
- Weight/style coverage: the menus use more weights than the HUD (bold
  headers, small caps). Any missing weight renders as the fallback or blank.

### B6. `safeRect` phone inset VALUES
Patch 0007 wires the API (`Stage.safeRect` narrows for phone notches) but
the values are **zero on desktop and unmeasured on the phone**. Nothing in
the engine reads the Android insets today: `android.rs` sets immersive
sticky flags via JNI (`getWindow()` / `getDecorView()`, android.rs:171-235)
but never queries `WindowInsets` / `DisplayCutout`. The JNI plumbing exists
— reading the cutout + gesture-bar insets through the same pattern is the
work. The app runs immersive sticky, so the insets are the camera cutout
and the gesture bar, not the status/navigation bars. This blocks correct
menu layout on the S25 Ultra (and every notched phone): without it the
menus draw under the cutout.

### B7. Input routing — touch → Ruffle
Ruffle takes `PlayerEvent` (mouse move/down/up, keyboard) via
`Player::handle_event` (player.rs:1057). There is no native touch path.
The engine must:
- Convert taps to mouse events at **stage coordinates** (1280×720 design
  space, scaled from the phone's physical pixels).
- Convert menu navigation to `NavigationCode` keys (TAB / UP / DOWN /
  ENTER) — the menus' `handleInput` is written for keyboard/gamepad
  navigation (`DialogueMenu.as` handles `NavigationCode.TAB` to close,
  UP/DOWN to move).
- Keep Bevy picking from double-consuming the same tap: the Bevy UI uses
  `PointerClick` observers; when a Ruffle menu is open, taps must go to
  the Ruffle player, not to Bevy nodes underneath. The touch UI (joystick
  + buttons) must hide when a Ruffle menu is open — the current UI fix
  round's menu gating covers the Bevy side; the Ruffle side needs the same
  gate.
- Drag input for the item-card 3D model (see B8) and for the map (pan/zoom).

### B8. `UpdateItem3D` — the 3D item model in the card
Shared by inventory, container, and magic. The SWF calls
`UpdateItem3D([true])` when the items list shows / an item is selected and
`UpdateItem3D([false])` when hidden (`ItemMenu.as:171,183,191` — identical
in all three menus). Note the call carries **only a boolean** — the host
must know the currently selected item itself (from its own `ItemSelect`
handling) and render that item's NIF model into the card region (740×410,
layer 2 in the Bevy recreation's `.swf`-exact layout).
- The engine must render a NIF model to a texture and composite it into
  the card area alongside the Ruffle frame. The Bevy inventory already
  renders the selected item's model with drag-rotate; the Ruffle version
  needs the same render, composited with the SWF frame instead of a Bevy
  UI node.
- Drag-rotate: the SWF gives no rotation input; rotation is host-side from
  drag events the host sees. The Bevy version proves the interaction; the
  Ruffle version needs the drag routed to the model render, not the SWF.

### B9. Save screenshots
The pause save/load panel and the start menu's load list show a screenshot
per save (`PrepSaveGameScreenshot` → host answers `ScreenshotReady`, the
SWF loads `img://BGSSaveLoadHeader_Screenshot`). The save system stores
JSON only (`save.rs`, `SaveData` v2) — **no screenshots exist**. Needs:
screenshot capture at save time (a render-target readback at the moment of
saving), storage (a sidecar file per save, or a `SaveData` field — the v3
bump already queued for death persistence is the natural vehicle), and the
`img://` provider (B4) to serve it.

### B10. Performance — a second player doubles the capture stall
The v0.4.13 phone logs show the Ruffle "capture" cost at 37–49 ms early in
a scene, for ONE player. **Correction (2026-11-17, Church, verified by
Jarvis — full report: `docs/ruffle-gpu-compositing-diagnosis.md`): this is
a GPU-DRAIN STALL, not a readback.** `capture_image`
(`vendor/ruffle/render/wgpu/src/utils.rs:131`) calls
`device.poll(wgpu::PollType::Wait)` (line 144), which blocks until ALL
submitted GPU work finishes — including the previous frame's whole Bevy
render graph. Pure readback is only ~2–5 ms; the 24–49 ms "capture" number
is the stall. A menu player capturing at 30 fps alongside the HUD player
would pay a second stall while a menu is open. Mitigation, in order of value:
1. **Pause the HUD player while a menu is open** — vanilla hides the HUD
   in menus anyway, so the HUD player should stop stepping and capturing
   the moment a menu opens. This is the obvious win and should be part of
   the first menu cutover, not a later optimization.
2. **GPU compositing is feasible with NO new Ruffle patch** (same report):
   one shared wgpu 30.0.2 device, the target texture already has
   `TEXTURE_BINDING` usage (`target.rs:245`), and
   `WgpuRenderBackend::target()` (`backend.rs:387`, pub) →
   `TextureTarget::get_texture()` (`target.rs:265`, pub) hands the texture
   out. Fix shape: a harness `hud_texture()` + a render-graph node between
   the 3D main pass and the UI pass + delete the capture/mailbox/re-upload
   path. Estimated: inn ~98 ms → ~55–65 ms, late exterior ~62 ms →
   ~50–55 ms. The remaining ~40 ms is the 3D main pass (2,216 sub-meshes
   outdoors, no batching/instancing) + the per-frame Bevy Image re-upload
   (3.7 MB, disappears with compositing) + inn-only point-light shadow
   faces (~9 ms). The menu player should be built on the composited path
   if M6 lands first; if M5 lands first, the menu player takes the
   capture path with the HUD paused (mitigation 1).
3. **State-gate the menu player.** `ruffle_hud_render_system` has NO state
   gate (ruffle.rs:406, `PrepareAssets`, no `run_if`) — the movie steps
   during door transitions (prev log window 52: render avg 105 ms, max
   396.6 ms across the +365 s transition). The menu player must only step
   and capture while its menu is open, or it pays the same cost during
   transitions.
4. Step the menu player at 30 fps only (the HUD already does this).

### B11. Quest data respond shapes (pause)
The journal asks the host for its data via `respond`, it does not read
injected lists: `RequestQuestsData([TitleList], this, "onQuestsDataComplete")`,
`RequestObjectivesData([])`, `RequestPlayerInfo([])` → `SetPlayerInfo`
(`QuestsPage.as:48,311`, `JournalBottomBar.as:42`). The host must answer
in the SWF's expected shapes (quest entry objects with title/stage/
completed flags; objective objects with text/completed). The engine has
the data (QUST parser, 339 journal quests) — the gap is the **shape** of
the answer, which must be read from `QuestsPage.as` during implementation.

## Per-menu gaps, in build order

### 1. Dialogue — `dialoguemenu.swf` — FIRST
**Bevy stand-in covers:** the full dialogue flow — topic list, subtitles,
speaker name, greeting/topic state machine, talk range (being fixed now),
dialogue-driven game-state changes. DIAL/INFO parsing exists (525 NPCs
with topics, 4,357 topics); **no condition evaluation — ALL topics show**
(documented, not faked).
**What the .swf needs:**
- Game→SWF (host calls, `DialogueMenu.as:32-40`): `SetSpeakerName(strName)`,
  `ShowDialogueText(astrText)`, `HideDialogueText()`,
  `PopulateDialogueList(...)` — variadic triples
  `(text, topicIsNew, topicIndex)` per topic plus a final arg = selected
  index (or `-1`); the handler pushes them into `TopicList.entryList`
  itself, so the host does **not** inject the list for dialogue.
  `ShowDialogueList(abNewList, abHideExitButton)`, `StartHideMenu`,
  `NotifyVoiceReady`, `Cancel`, `AdjustForPALSD` (PAL SD shift — irrelevant
  on a phone, but the handler exists).
- SWF→game (host answers): `TopicClicked([topicIndex])`,
  `SkipText([])`, `CloseMenu([])`, `FadeDone([])`.
- `NotifyVoiceReady` starts the progress timer (`ALLOW_PROGRESS_DELAY`) —
  with no audio system, the engine must decide when to fire it (presumably
  immediately on showing the line). This is the one dialogue-specific
  design decision.
- The host drives the menu state machine (TOPIC_LIST_SHOWN / TOPIC_CLICKED /
  SHOW_GREETING / TRANSITIONING) via the call order: populate → show list →
  show text → hide text → populate again. The Bevy dialogue's state machine
  is the reference for the order.
- Needs B2 (`$` tokens — the topic list and prompts are token-heavy), B3
  (keycaps on the exit button), B6 (safeRect), B7 (tap = click, TAB = close).
  No `img://`, no 3D model, no injected lists — the smallest bridge of any
  menu, which is why it goes first.
**Cutover risk:** the Bevy dialogue is the most-played menu; the Ruffle
version must match its topic coverage exactly (including the no-conditions
behavior — do not "fix" conditions as part of the cutover).

### 2. Pause — `quest_journal.swf` — SECOND
**Bevy stand-in covers:** Quests / Stats / System tabs (from
`quest_journal.swf`'s layout); 339 journal quests from the QUST parser;
working sensitivity stepper + invert-Y persisted to `settings.json`; Save
writes a real file; Load is wired through the save system. The study doc
`interface-extract/study/pause_hud_study.md` is the layout authority.
**What the .swf needs:**
- Quest data via respond (B11): `RequestQuestsData` /
  `RequestObjectivesData` / `RequestPlayerInfo` answers in the SWF's shapes.
- Save/load panel (`SaveLoadPanel.as`): SWF→game `IsOKtoLoad([index])` →
  host answers `ConfirmOKToLoad`; `PrepSaveGameScreenshot([index, entry])`
  → host answers `ScreenshotReady` and the SWF loads
  `img://BGSSaveLoadHeader_Screenshot` (B4 + B9). The save list itself is
  injected (save entries with name/date/level/playtime).
- System tab: the settings steppers (sensitivity, invert-Y) are host
  functions — the SWF calls the host to read/write them. The Bevy pause
  already persists these to `settings.json`; the Ruffle pause needs the
  same values through the bridge.
- Needs B2, B3, B4, B6, B7, B9, B11.
**Cutover risk:** the pause menu is the hub — the character menus (magic,
stats, map) open from it. The pause cutover must not break the Bevy
character-menu tabs that still live behind it until their own cutovers.

### 3. Inventory — `inventorymenu.swf` — THIRD
**Bevy stand-in covers:** the true `.swf` layout (categories x=20 + items
x=230 left, selected item's 3D model right at 740×410 layer 2 with
drag-rotate, item card at (532, 434.3), bottom bar y=645, 10 categories
incl. Scrolls, NO doll); pickup/drop/equip/use; gold; weight. The harness
`inventory` mode already drives the real SWF with canned data, and `upd`
drives `UpdateItemCardInfo` — the card rendering is proven in the harness.
**What the .swf needs:**
- Entry-list injection: the category list and the items list are injected
  (`ItemsList.entryList` etc.) in the SWF's shape — name, count, value,
  weight, damage/armor, type icon, equipped/favorited flags, stolen flag.
  The engine has all of it (`ItemDef` + inventory stacks); the gap is the
  **shape** of the injected objects, read from `InventoryMenu.as` /
  `ItemMenu.as` during implementation.
- Item card: `UpdateItemCardInfo` / `RequestItemCardInfo` — the card reads
  fields from the update object (`ItemCard.as:153` setter). The harness
  proves the card renders with a canned card; the gap is wiring the REAL
  selected item's data (damage, armor, effects, enchantment, description).
- `UpdateItem3D` (B8) — the 3D model in the card.
- SWF→game: `ItemSelect`, `ItemEquip`, `ItemUse`, `ItemDrop`, `ItemTake`,
  `ShowItem3D` / `HideItem3D` (the `UpdateItem3D` calls). The Bevy
  inventory implements all of these actions; the Ruffle version routes
  them through the bridge.
- Needs B2, B3, B6, B7, B8. No `img://` (item icons are in the SWF's own
  art / the game's icon textures — the icon path is part of the injected
  entry and needs checking against the harness's canned data).
**Cutover risk:** the inventory is the most data-dense menu; the injected
entry shape must match exactly or items render with missing fields. The
harness `inventory` mode is the place to prove the shape before the engine
cutover.

### 4. Container — `containermenu.swf` — FOURTH
**Bevy stand-in covers:** take-all looting only (`interact.rs:27-28` — "a
per-item container UI is a later milestone"). No per-item container UI
exists. **This menu has the least to regress — and the most new engine work.**
**What the .swf needs:**
- Reuses inventory's `ItemMenu` + `ItemCard` + `UpdateItem3D` (B8) — the
  inventory cutover unblocks most of the container's SWF side.
- Two injected lists: the player's items and the container's contents.
  The engine has the container contents (`catalog.container_contents`,
  `Containers` resource, `interact.rs:190`).
- SWF→game: `ItemTransfer([amount, isViewingContainer])`,
  `TakeAllItems([])`, `EquipItem([hand, amount])`, `XButtonPress`
  (`ContainerMenu.as:31-32,57,162,167,234`). Per-item transfer is new
  engine logic: move stacks between the player's inventory and the
  container, with the container's remaining contents persisted in
  `WorldChanges` — the save system records looted containers but not
  per-item container state, so this needs a `SaveData` extension (the v3
  bump already queued for death persistence is the natural vehicle).
- Needs B2, B3, B6, B7, B8.
**Cutover risk:** low on the UI side (take-all is the only behavior to
preserve), high on the data side (a half-looted container must persist
across scene loads and saves).

### 5. Magic — `magicmenu.swf` — FIFTH
**Bevy stand-in covers:** the Magic character tab — SPEL parser, the player
knows 5 spells incl. 2 passive Abilities; spell list with schools and
costs. Honestly unwired: spellcasting effect, shouts (empty), perks (0).
The study doc `interface-extract/study/magicmenu.txt` is the layout
authority.
**What the .swf needs:**
- Reuses inventory's `ItemMenu` + `ItemCard` + `UpdateItem3D` (B8).
- Entry-list injection: the spell list (name, school, cost, level,
  description, equipped-to-hand flags). The engine has the spell data
  (SPEL parser); the gap is the injected shape.
- SWF→game: `ItemSelect`, `EquipItem` (equip to left/right hand),
  `DragonSoulSpent` (shouts — the player has no shouts yet, so this is a
  stub until the shout system exists). Spellcasting itself is out of
  scope for the menu cutover — the menu equips; the casting system is a
  separate milestone.
- Needs B2, B3, B6, B7, B8.
**Cutover risk:** the Bevy magic tab is honest about what is unwired
(spellcasting, shouts, perks). The Ruffle menu must not promise what the
engine cannot do — equip works, casting does not (yet).

### 6. Favorites — `favoritesmenu.swf` — SIXTH
**Bevy stand-in covers: NOTHING.** Zero hits for "favorite" in the engine
outside the HUD's `abFavorMode` and one inventory comment ("Favorites is
the Q menu, not a category"). There is no favorites data, no favorites
UI, no hotkey system. **This menu is a from-scratch system, not a
cutover.**
**What the .swf needs:**
- Game→SWF (`FavoritesMenu.as:16-18`): `PopulateItems(...)`,
  `SetSelectedItem(...)`, `StartFadeOut(...)`.
- SWF→game: `ItemSelect([])` on item press (`FavoritesMenu.as:79`).
- The engine work is the favorites system itself: marking items/spells
  as favorites (from the inventory/magic menus — the Ruffle inventory's
  `ItemSelect` needs a "favorite" action), storing the favorites list
  (a `SaveData` field), and equipping from the favorites menu. The SWF
  is the simplest of all ten — the work is all host-side.
- Needs B2, B3, B6, B7.
**Cutover risk:** none (nothing to regress). The risk is scope: the
favorites system touches inventory, magic, and the save file.

### 7. Stats — `statsmenu.swf` — SEVENTH
**Bevy stand-in covers:** the Skills character tab — 18 skills with the
exact Nord values; the stats tab (level, health/magicka/stamina). Honestly
unwired: perks (0), shouts (empty). The study doc
`interface-extract/study/statsmenu.txt` is the layout authority.
**What the .swf needs:**
- Game→SWF (`StatsMenu.as:174-181`): `SetDescriptionCard(abPerkMode,
  aName, aMeterPercent, aDescription, aRequirements, aSkillLevel, aSkill,
  aLegendaryCount)`, `SetPlayerInfo(...)`, `UpdateSkillList(aCapitalizeSkillNames)`,
  `SetDirection(...)`, `HideRing(...)`, `SetStatsMode(abStats, aPerkCount)`,
  `SetPerkCount(aPerkCount)`, `ShowLegendaryButtonHint(...)`.
- The skill list is injected (`UpdateSkillList` reads the injected list).
  The engine has the skill data (18 skills, exact values).
- **Perks are the gap:** the stats menu's perk mode needs the perk tree
  (PERK records, the constellation layout, perk ranks, requirements).
  The engine has no perk system — perks are 0, and the Bevy tab is honest
  about it. The Ruffle stats menu can ship in stats mode (skills + level)
  with perk mode stubbed, or wait for the perk system. That is a scope
  decision for the parent: **stats-mode-only first, perk mode as a
  follow-up.**
- Legendary: `ShowLegendaryButtonHint` + the legendary count in
  `SetDescriptionCard` — the legendary system (skill reset at 100) does
  not exist. Stub until it does.
- Needs B2, B3, B6, B7.
**Cutover risk:** the Bevy skills tab is exact (Nord values). The Ruffle
version must show the same numbers.

### 8. Map — `map.swf` — EIGHTH
**Bevy stand-in covers:** the Map character tab — the real LAND bake
(1905×1505), 358 markers, player marker within 1px of Riverwood.
Honestly unwired: discovery state, indoor player marker.
**What the .swf needs:**
- `img://Local_Map` (B4) — the world image. **The pragmatic first step
  is serving the existing LAND bake as the `img://` image** — the Bevy
  map already produces it. A from-above rendered Tamriel (the "map needs
  to look like the actual map" item) is a separate visual pass.
- Marker injection: the marker list is injected in the SWF's shape
  (name, type icon, position, discovered/undiscovered). The engine has
  358 markers from the map bake.
- Game→SWF (`MapMenu.as:209-213`): `RefreshMarkers()`,
  `SetSelectedMarker(aiSelectedMarkerIndex)`,
  `ClickSelectedMarker()`, `SetDateString(astrDate)`,
  `ShowJournal(abShow)`.
- SWF→game: `MarkerClick` — the host must implement **fast travel** or
  decline it. The engine has no fast travel (no travel system, no
  loading-screen travel flow). This is a scope decision: the map can ship
  with travel disabled (markers selectable but not travelable) or wait
  for the travel system. Recommend shipping with travel declined and
  saying so plainly — the map's value is orientation, not travel.
- Needs B2, B3, B4, B6, B7 (pan/zoom drag).
**Cutover risk:** the Bevy map's marker accuracy (358 markers, player
marker within 1px) must carry over — the injected marker list must be
the same data.

### 9. Loading — `loadingmenu.swf` — NINTH
**Bevy stand-in covers:** the loading screen (logo + text + chunked-build
counts) that stays alive during the `Spawning` state. It works — the
v0.4.13 video shows it running for ~60-75 s on the exterior load.
**What the .swf needs:**
- SWF→game (host answers via `respond`): `RequestLoadingText([])` →
  `SetLoadingText(astrText)`; `RequestPlayerInfo([])` →
  `SetLevelProgress(afCurrentLevel, afLevelProgress, abShowMeter)`
  (`LoadingMenu.as:46,53,24-41`). No game→SWF callbacks at all.
- **The loading text source is unidentified.** The lore/tip text must
  come from the game data (likely LSCR records or the strings files) —
  identifying the source is implementation work.
- **The 3D model is the big gap.** In vanilla the loading screen's
  rotating model is rendered by the game and composited with the SWF
  frame — the SWF shows only text + the level bar. The engine must render
  a 3D model (the loading-screen model for the cell being loaded) and
  composite it with the Ruffle frame. This is the same compositing
  problem as B8, at full screen instead of a card.
- The Ruffle player must run **during the scene build** — the loading
  screen is shown in the `Spawning` state while `spawn_chunk` drains the
  budget. The current Bevy loading screen stays alive because it is Bevy
  UI; the Ruffle loading menu needs the player stepping and capturing
  while the main thread is busy building the scene. The chunked build's
  8 ms/frame budget leaves room for a 30 fps movie step — but the
  interaction between the build budget and the movie step needs
  measuring, not assuming.
- Needs B2, B6, B7 (no input needed — the loading screen takes none),
  B10 (the HUD player is not running during load, so the menu player is
  the only Ruffle cost — this menu is the cheapest on perf).
**Cutover risk:** the loading screen is the first thing the player sees
on every door transition. A broken loading menu is a broken game. This
menu goes late because its 3D-model gap is the largest single unknown.

### 10. Start — `startmenu.swf` — LAST
**Bevy stand-in covers: NOTHING.** There is no main menu — the app goes
straight to the loading screen and into the game. **This menu is a
from-scratch system, not a cutover, and it needs an AppState.**
**What the .swf needs:**
- A new `AppState::MainMenu` (the ChatGPT brief's Phase 5: the main menu
  is its own application state, not an overlay). The app currently starts
  in `Loading` and goes straight into the game; the start menu needs the
  app to stop at the menu and wait for the player's choice.
- Game→SWF (`StartMenu.as:88-100`): `sendMenuProperties(...)` →
  `setupMainMenu` (which items are enabled, version text, etc.),
  `ConfirmNewGame` / `ConfirmContinue` → `ShowConfirmScreen`,
  `FadeOutMenu` / `FadeInMenu`, `onProfileChange`, `StartLoadingDLC` /
  `DoneLoadingDLC` (the engine has no DLC concept beyond the 5 plugins —
  these need stub answers), `ShowGamerTagAndIcon` (needs `img://BGSUserIcon`,
  B4), `OnDeleteSaveUISanityCheck`, `OnSaveDataEventLoadSUCCESS` /
  `OnSaveDataEventLoadCANCEL`, `onStartButtonProcessFinished`.
- SWF→game: `NEW`, `CONTINUE`, `LOAD`, `SETTINGS`, `QUIT` (the menu
  choices) + the save-list flow (same `SaveLoadPanel` pattern as the
  pause menu — shared code, B9).
- **New Game stays UNWIRED** (Dylan's 2026-11-16 decision, re-affirmed
  against the ChatGPT brief). The menu item is present; the host's `NEW`
  handler must not start a new-game flow. What it does instead (show the
  confirm screen and then decline? ignore the press? show a "not
  available" note?) is a decision for the parent — the SWF expects an
  answer either way.
- `CONTINUE` needs the latest save (the save system has autosave +
  manual saves; the "latest save" is well-defined). `LOAD` needs the
  save list + screenshots (B9).
- Needs B2, B3, B4, B6, B7, B9, and the AppState work.
**Cutover risk:** none (nothing to regress). The risk is that the start
menu changes the app's boot flow — the first thing every player sees.
It goes last for that reason.

## Build order, ranked

| # | Menu | .swf | Why this position |
|---|------|------|-------------------|
| 1 | Dialogue | `dialoguemenu.swf` | Smallest bridge; no `img://`, no 3D model, no injected lists; data exists. Per the plan. |
| 2 | Pause | `quest_journal.swf` | Quest + save data exists; the hub for the character menus. Needs B9/B11. |
| 3 | Inventory | `inventorymenu.swf` | Harness-proven with canned data; unlocks container + magic (shared `ItemMenu`/`ItemCard`/`UpdateItem3D`). |
| 4 | Container | `containermenu.swf` | Reuses inventory's SWF side; adds per-item transfer + persistence. |
| 5 | Magic | `magicmenu.swf` | Reuses inventory's SWF side; spell data exists. |
| 6 | Favorites | `favoritesmenu.swf` | Simplest SWF; the work is the from-scratch favorites system. |
| 7 | Stats | `statsmenu.swf` | Skill data exists; perk mode is a follow-up (no perk system). |
| 8 | Map | `map.swf` | Needs B4 (`img://Local_Map`) + the fast-travel decision. |
| 9 | Loading | `loadingmenu.swf` | Needs the 3D-model composite + the loading-text source; must run during the scene build. |
| 10 | Start | `startmenu.swf` | Needs `AppState::MainMenu` + the boot-flow change; New Game stays unwired. |

## What to build first (the shared work, before menu 1)

1. **B1 — the menu-player slot.** One `Player` per menu, built on open.
   Measure the open cost on the phone before deciding on caching.
2. **B2 — `$` token resolution.** Blocks every menu. Decide the approach
   (Ruffle patch vs load-time rewrite) before menu 1.
3. **B10 — pause the HUD player while a menu is open.** Part of the first
   menu cutover, not a later optimization.
4. **B7 — touch → Ruffle input.** Tap-to-mouse + `NavigationCode` keys +
   the Bevy-picking gate.
5. **B6 — safeRect values.** Read the Android insets via the existing
   JNI plumbing in `android.rs`.
6. **B5 — fontlib once, shared.** Build it once; share it across players.
7. **B4 — `img://` provider.** Needed by map (8), pause save/load (2),
   start (10). Can land with the pause menu.
8. **B8 — `UpdateItem3D` composite.** Needed by inventory (3), container
   (4), magic (5). Land with the inventory menu.
9. **B9 — save screenshots.** Needed by pause (2) and start (10). Land
   with the pause menu.
10. **B3 — tag-1009 keycaps.** Needs the design decision (what does a
    Ruffle menu show where vanilla shows `[E]`?) before the engineering.

## Open questions for the parent

1. **Keycaps on touch (B3):** what does a Ruffle menu show where vanilla
   shows `[E]` / `[Tab]` / `[R]`? A tap target? Nothing? This decides
   whether tag-1009 rendering is even needed.
2. **New Game in the start menu:** the item is present but unwired. What
   does the host do when the player selects it — show the confirm screen
   and decline, ignore the press, or show a note?
3. **Fast travel (map):** ship the map with travel declined, or wait for
   the travel system? Recommendation: ship with travel declined, say so
   plainly.
4. **Perk mode (stats):** ship stats-mode-only first, perk mode as a
   follow-up? Recommendation: yes.
5. **Menu-open latency budget:** how long may opening a menu take on the
   phone before it needs prebuilt/cached players? This decides B1's
   caching strategy.
6. **`$` token approach (B2):** Ruffle patch vs load-time SWF rewrite.
   The patch is more invasive but keeps the SWF files untouched at load;
   the rewrite is contained but touches every string at load time.
7. **Loading text source:** identify where the lore/tip text lives in the
   game data (LSCR records? strings files?) before the loading menu.
