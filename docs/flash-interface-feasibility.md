# Flash Interface Feasibility — Run Skyrim's Real UI with Ruffle

**Date:** 2026-11-16 (study run 2026-11-15 22:19 – 2026-11-16 03:40 EST)
**Question (Dylan, 2026-11-15 22:18 EST):** use Skyrim's ACTUAL interface — "not the font, the HUD, all of it" — instead of Bevy-rebuilt approximations.
**Verdict: FEASIBLE, with one blocking upstream bug and three small Scaleform gaps — none of them fatal.**
Ruffle 0.7.1 renders the real HUD, dialogue menu, pause menu, and start menu today, in their real fonts, from the unmodified `.swf`/`.gfx` files. The inventory-family menus are blocked by exactly one known Ruffle bug (nested `ImportAssets`, issue #22426) with two known workarounds. The game↔menu API is fully mapped. The embedding story is concrete and the wgpu versions line up. This is a multi-milestone integration, not a drop-in — but nothing in the study says "cannot be done."

Read-only study: no `.rs` files were edited, the engine was not built, no cargo runs. All evidence below is from prebuilt Ruffle binaries, ffdec script exports, and Ruffle's own source on GitHub master.

---

## 1. What was proven by rendering (evidence)

Test harness: `interface-extract/tools/ruffle-test/run_test.sh` — Xvfb:99 + prebuilt Ruffle + ffmpeg x11grab screenshot at 7s, 1280×720. Ruffle 0.7.1-stable (`89a7049…`, 2026-10-07) and nightly 0.8.0 (`df1ec83…`, 2026-11-16) both used. Screenshots in `interface-extract/tools/ruffle-test/shot-*.png`.

| Menu | File | Result in Ruffle 0.7.1 | Screenshot (md5) |
|---|---|---|---|
| HUD | `study/hudmenu.gfx` | **Renders.** Real compass (ornate frame, "N" centred), "Location" top-right, dialogue subtitle lines centre-bottom, correct Futura font. Vital bars hidden (correct — no game data). | `shot-hud-fontlib.png` d1b3db28… |
| Dialogue | `interface/dialoguemenu.swf` | **Renders.** Topic list right, "Selected Topic" + diamond, scrollbar, "Selected Item", subtitle lines, fading black panel. Bottom-right shows raw green/blue shapes + literal "$Exit" (see §4: tag 1009 + `$` translation). | `shot-dlg-fontlib.png` bf183813… |
| Pause (journal) | `study/quest_journal.swf` | **Renders.** Tab bar "$QUESTS / $GENERAL STATS / $SYSTEM", big panel with corner ornaments, bottom bar, "$LEVEL" + blue level meter, "Time, Date, Year". Matches the prior `.swf` study exactly. | `shot-journal-fontlib.png` c3be1c12… |
| Start (main menu) | `interface/startmenu.swf` | **Renders.** Bethesda Game Studios vector logo, "$DeleteUser", "<version>". Main list empty (game-driven). | `shot-start-fontlib.png` 241be744… |
| Map | `interface/map.swf` | **Renders empty — expected.** The map is almost entirely game-driven (`img://Local_Map` image + marker callbacks); with no game data there is nothing to show. | `shot-map-fontlib.png` e136b2bc… |
| Inventory | `interface/inventorymenu.swf` | **BLOCKED.** Stage bg (51,51,51) only; no lists, no item card, no bottom bar. Cause: nested-import bug, §4. | `shot-inv-fontlib2.png` 2913cc67… |
| Magic / Container | `interface/magicmenu.swf` / `containermenu.swf` | **BLOCKED — same bug.** Same component-import structure as inventory. (Not screenshot-tested individually; same imports, same failure mode.) | — |

**All five rendering menus needed one thing first: the font library** (§4.1). Without it every menu renders a blank white stage — Ruffle loads the movie, runs frame-1 setup, then goes silent (no frames submitted, not even the `SetBackgroundColor`). With the synthesized `gfxfontlib.swf` present, frames render.

### 1.1 ActionScript version — proven AS2

Tag survey of 13 files (hudmenu, map, inventory, dialogue, fonts_en, start, magic, stats, loading, sharedcomponents, quest_journal, favorites, container): **zero `DoABC` tags in all files**; heavy `DoAction`/`DoInitAction` (map.swf: 1,286 sprite DoActions; hudmenu: 64 sprite DoActions + 38 root DoInitActions). SWF version 15 container, AS2 bytecode — the classic Scaleform profile. Ruffle's AVM1 interpreter runs it (the renders above are the proof — the frame-1 scripts execute, `stop()`, and wait for the game).

The CLIK/gfx framework (`__Packages/gfx/...`: UIComponent, FocusHandler, InputDelegate, Button, ScrollBar…) is **compiled into every menu SWF** — Ruffle doesn't need to implement `gfx.*` classes, just run the AS2 bytecode. ffdec exported **2,150 `.as` files** into `interface-extract/flash-exports/scripts/` (hudmenu 104, map 1,380, quest_journal 232, startmenu 112, inventory 66, magicmenu 53, statsmenu 51, containermenu 56, dialoguemenu 43, favoritesmenu 41, loadingmenu 12).

Frame-1 scripts (read in inventorymenu + hudmenu): define game-called globals (`InitExtensions`, `SetPlatform`, `GetInventoryItemList`), set `_global.gfxExtensions = true`, call `Shared.GlobalFunc.MaintainTextFormat()`, `Menu_mc.SetFadedIn()`, then `stop()`. **The movies stop on frame 1 and wait for the game to drive them** — exactly the host-driven model Ruffle's `Player` API supports (§5).

---

## 2. The game↔menu bridge — the integration surface

### 2.1 Mechanism (proven from decompiled code)

`gfx/io/GameDelegate.as` is identical in every menu. The entire bridge is `flash.external.ExternalInterface` with **two entry points**:

- **SWF → game:** `GameDelegate.call(methodName, params, scope, callBack)` → `ExternalInterface.call(methodName, uid, ...params)` — a numeric UID is prepended so responses can be matched.
- **Game → SWF:** `GameDelegate.initialize()` registers exactly two ExternalInterface callbacks: `"call"` (→ `receiveCall` → dispatches to handlers registered via `addCallBack`) and `"respond"` (→ `receiveResponse` → matches a UID to a pending callback).

No `fscommand` anywhere (0 hits in 2,150 files). `ExternalInterface` appears only in the framework (`gfx/core/UIComponent.as`, `gfx/io/GameDelegate.as`), never in menu-specific code. So the host implements **one dispatch function + one invoke function**, and the method-name vocabularies below are the whole contract.

There is a **third channel: variable injection.** The game sets variables directly on movie clips (Scaleform's `Movie::SetVariable`):

- `BSScrollingList.entryList` — the items array of every scrolling list (getter/setter property, `BSScrollingList.as:234-238`). No AS code in the menus assigns it; no `dataProvider`; no `_global.` data passing. The game injects it.
- `HUDMenu.CompassTargetDataA` — initialized as an empty array (`HUDMenu.as:146`), never assigned in AS. `SetCompassMarkers()` reads it as a flat array of quads: `[heading, alpha, frame, scale]` per marker.

Ruffle has no public `set_variable` API, but `Player::mutate_with_update_context` (§5) exposes the stage tree and the AVM1 object API to the host — variable injection is implementable through it (walk `stage → root clip → named child`, `Object::set`). Flagged as fiddly-but-feasible, not a blocker.

### 2.2 Per-menu API tables

Counts are exact (grep over the exported `.as` files). **Arg shapes are given where I read the handler body; where I only enumerated the name, that is marked `[args unread]`** — the `.as` files are all exported, so finishing the arg survey is grep + read work during implementation, not new research.

#### HUD — `hudmenu.gfx` (`HUDMenu.as`, 1,030 lines; signatures read)

SWF → game (3): `PlaySound(soundName)` · `RegisterHUDComponents()` · `GetButtonFromUserEvent(userEvent)` [args unread]

Game → SWF (27 callbacks):
| Callback | Args (from handler body) |
|---|---|
| `SetHealthMeterPercent` / `SetMagickaMeterPercent` / `SetStaminaMeterPercent` / `SetShoutMeterPercent` | `(aPercent, abForce)` |
| `SetChargeMeterPercent` | `(aPercent, abForce, abLeftHand, abShow)` |
| `SetCompassAngle` | `(aPlayerAngle, aCompassAngle, abShowCompass)` — lerps the compass strip |
| `SetCompassMarkers` | `()` — reads injected `CompassTargetDataA` (quads: heading, alpha, frame, scale) |
| `SetCompassTemperature` | `(aTemperatureLevel, abForce)` — Survival Mode |
| `SetCrosshairTarget` | `(abActivate, aName, abShowButton, abTextOnly, abFavorMode, abShowCrosshair, aWeight, aCost, aFieldValue, aFieldText)` |
| `SetCrosshairEnabled` / `ValidateCrosshair` | `(abFlag)` / `()` |
| `SetLoadDoorInfo` | `(abShow, aDoorName)` |
| `SetLocationName` | `(aLocation)` |
| `SetEnemyHealthPercent` / `SetEnemyHealthTargetPercent` | [args unread — presumably `(aPercent)`] |
| `ShowElements` | `(aMode, abShow)` — HUDModes stack; elements keyed by mode name |
| `ShowMessage` / `ShowSubtitle` / `HideSubtitle` / `ShowNotification` / `ShowTutorialHintText` | `(asMessage)` / `(astrText)` / `()` / [unread] / `(astrHint, abShow)` |
| `SetSubtitlesEnabled` | `(abEnable)` |
| `StartMagickaMeterBlinking` / `StartStaminaMeterBlinking` | `()` |
| `FadeOutStamina` / `FadeOutChargeMeters` / `FlashShoutMeter` | `(aPercent)` / `()` / `()` |

Also in the class (Survival Mode, AE content): `ShowSurvivalElements(abShow, aUpdateData, abForce)`, `SetColdPenaltyMeter` / `SetHungerPenaltyMeter` / `SetExhaustionPenaltyMeter(aPercent, abForce)`, `ShowArrowCount(aCount, abHide, aArrows)`. The HUD's auto-hide behaviour Dylan asked for ("health bars should only show up when used") is **implemented inside this AS** (meters fade via `FadeOutMeter`/`FadeOutStamina` on a timer) — the host just reports percents; the SWF owns the fade.

#### Inventory — `inventorymenu.swf`

SWF → game (17): `PlaySound` · `UpdateItem3D` · `RequestItemCardInfo` · `ItemSelect` · `CloseMenu` · `ShowTweenMenu` · `ZoomItemModel` · `StopMouseRotation` · `StartMouseRotation` · `ShowSoulGemList` · `SaveIndices` · `QuantitySliderOpen` · `ItemDrop` · `ItemCardListCallback` · `DisabledItemSelect` · `CloseTweenMenu` · `CheckForMouseEquip` [args unread except where noted in §2.3]

Game → SWF (11): `AttemptChargeItem` · `AttemptEquip` · `DropItem` · `InvalidateListData` · `ItemRotating` · `RestoreIndices` · `SetCategoriesList` · `ToggleMenuFade` · `UpdateItemCardInfo` · `UpdatePlayerInfo`
- `SetCategoriesList(...)` — **variadic triples**: `(text, flag, bDontHide)` per category (`InventoryLists.as:250`); `flag == 0` means a divider row.
- `InvalidateListData()` — re-derives which categories are visible from the injected items' `filterFlag`s.
- `UpdatePlayerInfo(aUpdateObj)` → BottomBar (gold, carry weight, etc.); `UpdateItemCardInfo(aUpdateObj)` → `ItemCard_mc.itemInfo` + bottom bar per-item info.
- Items arrive by **variable injection** into `ItemsList.entryList` (§2.1).
- `InitExtensions()` registers the AttemptEquip/DropItem/AttemptChargeItem/ItemRotating callbacks; `onFadeCompletion` calls `CloseMenu`. Bottom-bar button art maps per platform (e.g. Equip: PC `"M1M2"`, Xbox `"360_LTRT"`, PS3 `"PS3_LTRT"`).

#### Map — `map.swf`

SWF → game (7): `PlaySound` · `ToggleMapCallback` · `SetLocalMapExtents` · `OpenJournalCallback` · `MarkerClick` · `CurrentLocationCallback` · `ClickCallback`
Game → SWF (5): `ClickSelectedMarker` · `RefreshMarkers` · `SetDateString` · `SetSelectedMarker` · `ShowJournal`
Plus: the map image arrives via `img://Local_Map` (`Map/LocalMap.as:47` — `MapImageLoader.loadClip("img://Local_Map", TextureHolder)`), i.e. the game renders the world from above and hands the SWF an image. This is the real Skyrim map mechanism — and it is also the answer to Dylan's "the map needs to look like the actual map": with Ruffle, the map tab shows the live 3D world from above, as the game does.

#### Dialogue — `dialoguemenu.swf`

SWF → game (4): `TopicClicked` · `SkipText` · `FadeDone` · `CloseMenu`
Game → SWF (9): `AdjustForPALSD` · `Cancel` · `HideDialogueText` · `NotifyVoiceReady` · `PopulateDialogueList` · `SetSpeakerName` · `ShowDialogueList` · `ShowDialogueText` · `StartHideMenu`

#### Favorites — `favoritesmenu.swf`

SWF → game (3): `PlaySound` · `ItemSelect` · `FadeDone`
Game → SWF (3): `PopulateItems` · `SetSelectedItem` · `StartFadeOut`

#### Pause — `quest_journal.swf` (the big one)

SWF → game (~50): `PlaySound` · `SetSaveDisabled` · `ToggleQuestActiveStatus` · `RequestInputMappings` · `QuitToMainMenu` · `PrepSaveGameScreenshot` · `OptionChange` · `LoadGame` · `DeleteSave` · `CharacterSelected` · `updateStats` · `UseCurrentCharacterFilter` · `ToggleShowMiscObjectives` · `StartRemapMode` · `ShowTargetOnMap` · `ShouldShowMod` · `ShouldShowKinectTunerOption` · `SetVersionText` · `SaveSettings` · `SaveGame` · `SaveControls` · `SAVE` · `ResetControlsToDefaults` · `RequestQuestsData` · `RequestPlayerInfo` · `RequestObjectivesData` · `RequestIsOnPC` · `RequestHelpText` · `RequestGameplayOptions` · `RequestDisplayOptions` · `RequestAudioOptions` · `RememberCurrentTabIndex` · `QuitToDesktop` · `QuickSave` · `PopulateHelpTopics` · `PopulateCharacterList` · `OpenKinectTuner` · `ORBISDeleteSave` · `ModManager` · `LOAD` · `IsOKtoLoad` · `GetButtonFromUserEvent` · `ForceStopSaveListLoading` · `CloseMenu`
Game → SWF (~20): `BackOutFromLoadGame` · `ConfirmOKToLoad` · `FinishRemapMode` · `HideMenu` · `OnSaveDataEventLoadCANCEL` · `OnSaveDataEventSaveCANCEL` · `OnSaveDataEventSaveSUCCESS` · `RefreshSystemButtons` · `RestoreSavedSettings` · `ScreenshotReady` · `SetRemoteDevice` · `SettingsSaved` · `ShowMenu` · `StartCloseMenu` · `UpdatePermissions` · `onFillCharacterListComplete` · `onRightStickInput` · `onSaveLoadBatchComplete`
Note: save/load is request/response over the UID channel (`IsOKtoLoad` → `ConfirmOKToLoad`; `PrepSaveGameScreenshot` → `ScreenshotReady`; save screenshots arrive via `img://BGSSaveLoadHeader_Screenshot`).

#### Start / main menu — `startmenu.swf`

SWF → game (~25): `PlaySound` · `QuitToDesktop` · `PrepSaveGameScreenshot` · `DeleteSave` · `fadeOutStarted` · `QuitToMainMenu` · `PopulateCharacterList` · `OpenMarketplace` · `OnDisabledLoadPress` · `ORBISDeleteSave` · `NEW` · `MOD` · `LoadDLC` · `IsOKtoLoad` · `HELP` · `ForceStopSaveListLoading` · `EndPressStartState` · `DoLoadDLCPlugins` · `DoDeleteSaveUISanityCheck` · `CharacterSelected` · `ChangeUser` · `CONTINUE`
Game → SWF (~17): `ConfirmContinue` · `ConfirmNewGame` · `ConfirmOKToLoad` · `DoneLoadingDLC` · `FadeInMenu` · `FadeOutMenu` · `OnDeleteSaveUISanityCheck` · `OnSaveDataEventLoadCANCEL` · `OnSaveDataEventLoadSUCCESS` · `ScreenshotReady` · `ShowGamerTagAndIcon` · `StartLoadingDLC` · `onFillCharacterListComplete` · `onProfileChange` · `onSaveLoadBatchComplete` · `onStartButtonProcessFinished` · `sendMenuProperties`
Plus `img://BGSUserIcon` (`StartMenu.as:216`).

#### Magic — `magicmenu.swf`

SWF → game (13): inventory's set minus container-only calls, plus `ShowShoutFail`.
Game → SWF (9): inventory's set plus `DragonSoulSpent` (no `DropItem`/`AttemptChargeItem`/`ItemRotating`).

#### Stats / skills — `statsmenu.swf`

SWF → game (2): `MoveCamera` · `SetFadedIn`
Game → SWF (9): `HideRing` · `SetDescriptionCard` · `SetDirection` · `SetPerkCount` · `SetPlayerInfo` · `SetStatsMode` · `ShowLegendaryButtonHint` · `UpdateSkillList`

#### Loading — `loadingmenu.swf`

SWF → game (2): `RequestPlayerInfo` · `RequestLoadingText` — both answered via the `"respond"` UID channel (no `addCallBack`s registered at all).
Game → SWF: none. (This is the OG loading screen: 3D model + lore text. The model is rendered by the game and composited; the SWF shows the text.)

#### Container — `containermenu.swf`

SWF → game (15): inventory's set plus `TakeAllItems` · `ItemTransfer` · `EquipItem`, minus `ShowTweenMenu`/`ShowSoulGemList`/`ItemDrop`/`ItemCardListCallback`/`CloseTweenMenu`.
Game → SWF (9): inventory's set plus `XButtonPress` (no `AttemptChargeItem`/`DropItem`/`ItemRotating`).

### 2.3 What the survey did NOT finish

Per-call argument lists for the ~80 SWF→game methods were enumerated by name, not read call-site by call-site (the HUD's game→SWF handlers and the inventory list/category/player-info paths were read in full; the rest were not). During implementation, each method's args come from a grep of the exported `.as` files, e.g. `grep -rn 'GameDelegate.call("ItemSelect"'` + reading the surrounding lines. The files are all in `interface-extract/flash-exports/scripts/` — this is mechanical, not research. The `addCallBack` list above is complete (verified: the earlier truncated output was re-run per-menu).

M3 update (2026-11-16): the inventory drive proved three SWF→game
calls live — `PlaySound("UIMenuBladeOpenSD")`,
`UpdateItem3D(true)` (the SWF asks the game to show the 3D item
model), `RequestItemCardInfo(2)` — and the `ItemCard.as` /
`BottomBar.as` / `InventoryMenu.as` / `InventoryLists.as` handlers
were read in full to build the drive (item-card fields, player-info
fields, category triples, the `ShowItemsList` → `showItemsList` →
`RequestItemCardInfo` → `UpdateItemCardInfo` flow). The remaining
unread methods are the ones no drive has exercised yet.

---

## 3. How Ruffle embeds in a Bevy/wgpu app

All API claims below were read from Ruffle master on GitHub (2026-11-16). File references are `ruffle-rs/ruffle` master.

### 3.1 The pieces

| Piece | Ruffle API | What it does for Dovahengine |
|---|---|---|
| Player | `ruffle_core::Player`, `PlayerBuilder` (`core/src/player.rs:4846`) | Host constructs with `with_movie(SwfMovieData)`, `with_renderer`, `with_navigator`, `with_external_interface`, `with_viewport_dimensions`, `with_frame_rate`, `with_load_behavior`, `with_letterbox`, `with_scale_mode`/`with_align`. Null backends are the default — the host supplies only what it needs. |
| Per-frame step | `Player::run_frame()` (`player.rs:3757`), `Player::render()` (`player.rs:3828`) | Called once per Bevy frame (or at the movie's 30 fps). `run_frame` preloads, runs AVM1/AVM2 phases, audio, post-frame callbacks; `render` builds the command list and calls `renderer.submit_frame(...)`. `needs_render` tracks dirtiness. |
| SWF → game | `ExternalInterfaceProvider` trait (`core/src/external.rs:674`): `call_method(&self, context, name, args) -> Value`, `on_callback_available(name)`, `get_id()` | The host implements the ~80-method GameDelegate dispatch here. `PlayerBuilder::with_external_interface(provider)` / `Player::set_external_interface_provider` (`player.rs:4623`). |
| Game → SWF | `Player::call_internal_interface(name, args)` (`player.rs:4640`) | Invokes the SWF-registered ExternalInterface callback by name — i.e. `call_internal_interface("call", ["SetHealthMeterPercent", 0.75, false])` reaches `GameDelegate.receiveCall` → the handler. `"respond"` with a UID answers a SWF request. |
| Value marshalling | `ruffle_core::external::Value` (`external.rs:43`): `Undefined/Null/Bool/Number(f64)/String/Object(BTreeMap)/List(Vec)` | Lossy-but-sufficient: numbers are f64 (u32 form IDs are exactly representable), objects and arrays convert both ways to/from AVM1 (`from_avm1`/`into_avm1`). MovieClips become Null — the menus never pass those across. |
| Fetch interception | `NavigatorBackend` trait (`core/src/backend/navigator.rs:491`): `fetch(&self, request) -> OwnedFuture<...>`, `resolve_url`, `spawn_future`, `pre_process_url` | **The host controls every fetch.** This is where gfxfontlib serving, case-insensitive import resolution, and `img://` images are implemented (§4). `Player::fetch` also applies `compatibility_rules` URL rewrites. Fetch is async — the host must pump the future executor each frame (the desktop player does this via its event loop; the `frontend-utils` crate may hold reusable pieces). |
| Variable injection | `Player::mutate_with_update_context(f)` (`player.rs:4194`) | Gives the host an `UpdateContext` (stage tree, library, AVM1) — the route for setting `entryList` / `CompassTargetDataA` on clips (§2.1). No public `set_variable` exists; this is the supported escape hatch. |
| Input | `Player::handle_event(PlayerEvent) -> bool` (`player.rs:1957`) | `KeyDown/KeyUp`, `MouseMove/MouseDown/MouseUp/MouseWheel`, `GamepadButtonDown/Up` (mapped to keys via `PlayerBuilder`'s `gamepad_button_mapping`), `TextInput/TextControl/Ime`, `FocusGained/FocusLost`. Returns whether the event was handled. **No native touch event** — Bevy touch maps to mouse (down/move/up at stage coords). The menus are mouse-capable (PC mode), so touch works through the CLIK mouse path. |
| Transparency | `Player::set_window_mode("transparent")` (`player.rs:1900`) | `render()` then clears to RGBA 0 instead of the stage bg color (`player.rs:3899`) — required for the HUD overlay over the 3D scene. |
| Fonts | `Player::register_device_font` (`player.rs:4721`) | Host can register device fonts; not needed — the menus bring their own via the fontlib. |

### 3.2 Rendering: render-to-texture is first-class, and the wgpu versions match

- **Ruffle master uses wgpu 30.0.1** (workspace `Cargo.toml:114`). **Bevy 0.20.1 uses wgpu 30.0.2** (Dovahengine `Cargo.lock`). Same major version — Cargo unifies them; the two crates can share one `wgpu::Device`/`Queue`. (This was the biggest unknown going in. It resolved favorably.)
- `WgpuRenderBackend::new(descriptors: Arc<Descriptors>, target: T)` (`render/wgpu/src/backend.rs:430`) takes an existing device/queue via `Descriptors::new(instance, adapter, device, queue)` — the host is not forced onto Ruffle's own instance. Bevy exposes `RenderDevice`/`RenderAdapter` (and `RenderInstance`) from the render world; constructing `Descriptors` from them is the integration point (verify exact resource names against Bevy 0.20 at implementation time).
- `RenderTarget` is a public trait (`render/wgpu/src/target.rs:25`), with two built-in implementations:
  - `TextureTarget` (`target.rs:354`): renders into a `wgpu::Texture` (`Rgba8Unorm`, usage `RENDER_ATTACHMENT | COPY_SRC | TEXTURE_BINDING`), `get_texture()` returns a cloneable handle, and `WgpuRenderBackend<TextureTarget>::capture_frame()` (`backend.rs:395`) returns an `image::RgbaImage` — the exporter's CPU-readback path.
  - `SwapChainTarget`: renders to a window surface (the desktop player).
- **Compositing options, in order of preference:** (a) render Ruffle into a `TextureTarget` on Bevy's device and sample that texture on a fullscreen UI quad — needs a small custom Bevy render node or a Bevy `Image` wrapping the external texture (unproven, needs a spike); (b) CPU readback via `capture_frame()` + upload as a Bevy `Image` — proven by Ruffle's `exporter` package, simple, costs one 1280×720×4 ≈ 3.7 MB copy per dirty frame (only re-render when `needs_render`); (c) a custom `RenderTarget` impl that renders directly into a Bevy-managed texture view (the trait is public; the host controls submission).
- Flash colors/blending are done in sRGB space — Ruffle deliberately uses non-sRGB target formats (`target.rs:122-128`). The Bevy quad must sample the texture as-is (no additional sRGB conversion) or colors will double-convert.
- Ruffle also ships an `exporter` package (headless PNG screenshots of a SWF) — usable for a regression harness that renders menu frames without a window.

### 3.3 Depending on Ruffle

- **Ruffle crates are NOT published to crates.io** (`crates.io/crates/ruffle_core` 404s, 2026-11-16). Embedding means a **git dependency** (`ruffle_core = { git = "https://github.com/ruffle-rs/ruffle" }`, same for `ruffle_render`, `ruffle_render_wgpu`, `swf`) or a vendored copy. Pin to a tested commit (0.7.1 tag `89a7049…` or newer). A fork is likely anyway (§4 — the nested-import fix and the small Stage patch live in Ruffle's tree unless upstream lands them).
- **License: MIT OR Apache-2.0** (workspace `Cargo.toml:89`) — clean for Dovahengine. No GPL anywhere in the Ruffle tree.
- **Edition 2024** (workspace `Cargo.toml:85`) — needs Rust ≥ 1.85. Local toolchain is Cargo 1.100. No `rust-toolchain` pin found at the Ruffle repo root.
- **Android viability:** `ruffle_core` + `ruffle_render` + `ruffle_render_wgpu` + `swf` are platform-agnostic Rust. The desktop-only pieces (winit window, cpal audio, rfd dialogs) are not needed — the host supplies null/stub backends (audio: `NullAudioBackend` exists; video: menus have no video, stub it). wgpu on Android (Vulkan) is the same path Bevy already uses. Open questions, not blockers: binary-size impact of AVM1+AVM2+render (measure at M1), and AVM1 interpreter CPU cost on a phone CPU (the menus are light — mostly static frames with occasional tweens; the HUD updates a handful of numbers per frame).
- **Precedent:** FlashNX (Switch homebrew) embeds `ruffle_core` + `ruffle_render` with a custom OpenGL/GLES backend — embedding with a non-wgpu renderer is a known pattern; Dovahengine's case (wgpu, same major version) is easier than that.

---

## 4. Scaleform-specific features — coverage and gaps

| Feature | Where it appears | Ruffle coverage | Resolution |
|---|---|---|---|
| **Shared font library (`gfxfontlib.swf`)** | Every menu: `ImportAssets2` (tag 71) imports `$EverywhereMediumFont` etc. from `gfxfontlib.swf` | **Not implemented as a concept** — Ruffle treats it as a literal relative URL. Vanilla Skyrim ships no such file: the game registers `fonts_en.swf` via `fontconfig.txt`'s `fontlib` directive and Scaleform intercepts the URL internally. | **SOLVED in this study.** A synthesized `gfxfontlib.swf` was built (12 `DefineFont3` tags copied verbatim from `fonts_en.swf` + one `ExportAssets` mapping the 15 `$` names per `fontconfig.txt`): `tools/ruffle-test/gfxfontlib.swf`, 487,761 B, md5 `44591911…`. With it present, menus render. In production: build it once at build time (from the player's own `fonts_en.swf`) or serve it from memory via the `NavigatorBackend` fetch interception. Font names in the SWF carry a trailing NUL — strip it when matching. |
| **Nested `ImportAssets2`** | inventory/magic/container import `ItemCard.swf` / `InventoryLists.swf` / `BottomBar.swf`, which themselves import `$EverywhereMediumFont` from `gfxfontlib.swf` | **BROKEN — upstream issue #22426** ("Preload terminates early when processing nested imports", opened 2025-12-06, no fix linked on the issue page as of 2026-11-16). Ruffle preloads the imported movie exactly once; a nested import before the `ExportAssets` terminates the preload early, so the export never registers. Reproduced on nightly 2026-11-16: `Character ID collision: Tried to register ID 37 twice` (components' internal IDs collide with the main movie's in Ruffle's shared library) + `Tried to instantiate a non-registered character ID 87/89/90` (the three components). `--load-behavior blocking` does not fix it. | **The one blocking bug.** Two workarounds: **(a)** fix Ruffle (recursive preload in `loader.rs` + per-movie ID namespacing — the issue describes the fix shape; needs a Ruffle contributor's care, but the repro is minimal and in-repo); **(b)** build-time SWF surgery — merge the three component SWFs into each menu SWF with remapped character IDs, replacing `ImportAssets2` with local definitions (a repack step in Dovahengine's asset pipeline; no Ruffle change). Decide at M3 after the HUD milestone proves the rest. |
| **`DefineExternalImage2` (tag 1009)** | Keycap/button icons (`360_A`, `E`, `Tab`, …) — external images from `interface/exported/*.png.dds` | **Not implemented** — Ruffle warns `Unknown tag code: 1009` (~146× in hudmenu) and `Export asset: AD.png … not found in library`. The dialogue menu's bottom bar shows raw green/blue shape fills where the keycap should be. | Two options: **(a)** implement tag 1009 in Ruffle + a host image provider (Dovahengine already decodes DDS — serve the decoded pixels); **(b)** build-time SWF surgery — replace each `DefineExternalImage2` with an embedded `DefineBitsLossless2` (decoded DDS pixels). (a) is cleaner long-term; (b) is a repack step. Either way the icons are recoverable — the DDS files exist. |
| **`$` translation tokens** | Static text everywhere: `$QUESTS`, `$Exit`, `$DeleteUser`, `$LEVEL`, `$Toggle Active`… | **No translator hook.** In Scaleform, `$token` strings are translated by the host's `GFx::Translator` (lookup in `translate_english.txt`); Ruffle renders them literally (visible in the dialogue/pause/start screenshots). `SetText` in AS just sets text — translation is not done in ActionScript. | Two options: **(a)** patch Ruffle with a translator hook (intercept `TextField.text` assignment, translate `$` tokens via a host callback backed by `translate_english.txt`); **(b)** build-time repack — replace `$TOKEN` strings in the SWFs' `DefineEditText` initial text and AS constant pools with the English text. (a) is the right fix (one small hook); (b) is feasible but must cover AS-constructed strings too. Note: English-only is fine — the game ships `translate_english.txt`. |
| **`img://` image provider** | `img://Local_Map` (map), `img://BGSSaveLoadHeader_Screenshot` (pause/start save lists), `img://BGSUserIcon` (start menu) | **Not implemented** (Scaleform-specific protocol). | Serve via the host `NavigatorBackend`: intercept `img://` URLs in `fetch()` and return generated PNG/JPEG bytes (map render, save screenshots, user icon). The map image is the live 3D world from above — Dovahengine renders it. |
| **`Stage.visibleRect` / `Stage.safeRect`** | `GlobalFunc.SetLockFunction` (HUD element anchoring: 13 `Lock()` calls at HUD init — meters "B"/"BL"/"BR", compass "T", location "TR", …) and `ItemMenu.as:66-81` | **Not implemented** — Ruffle's AVM1 Stage object (`core/src/avm1/globals/stage.rs`) exposes only `align/scaleMode/height/width/showMenu/displayState`. **CORRECTION (M2): the study's claim that "menus lay out correctly at 1280×720 anyway" was wrong** — it rested on the no-data screenshots, where `InitExtensions` never ran and `Lock()` never executed. Once `InitExtensions` runs, `Lock` computes NaN anchors (undefined rects); Ruffle's `globalToLocal` coerces NaN to 0 twips (Rust `as i32` saturating cast), so every anchored clip parks at global (0, 0) — the meter art hangs above its clip origin and renders off the top edge, the location text off the left edge. Only the compass (art below its origin) stayed visible. This was the M2 root cause for the missing meters/location. | **SOLVED in M2** by `vendor/ruffle/0003-stage-visible-rect.patch` — two Stage getters returning the full stage rect as `flash.geom.Rectangle` (`safeRect` = same rect while there is no inset; the phone build can narrow it for notch/gesture bars later). After the fix the HUD lays out exactly as in the game (meters bottom, location top-right). |
| **`gotoAndStop`/`gotoAndPlay` with float frame numbers** | `Components.Meter.Update` passes raw `Lerp` floats to `gotoAndStop` (only `SetPercent` floors); any AS that gotos a computed frame | **Divergent** — stock Ruffle's `goto_frame` only runs a direct goto `if n.fract() == 0.0`; a float falls through to the label branch, where the stringified number ("6.97") fails path resolution and the goto silently does nothing. Flash truncates (ECMA ToInt32). Found in M2: the bars tweened their percents correctly (`CurrentPercent` 100→75) but the bar clip never moved off its Full frame. | **SOLVED in M2** by `vendor/ruffle/0004-goto-float-frame.patch` — drops the `fract` guard so every `Value::Number` truncates via `f64_to_wrapping_i32`; NaN/inf/negatives still no-op via the `frame > 0` check. Genuine upstream divergence, worth proposing. |
| **`_global.gfxExtensions = true`** | Frame 1 of every menu | Harmless flag — AS reads it in a few places (e.g. input handling branches); Ruffle runs the AS as-is. | None. |
| **Case-sensitive import URLs** | Import URLs use `Inventory components/ItemCard.swf` (capital I/C); on-disk files are lowercase (`itemcard.swf`, …) | Ruffle resolves URLs literally — fails on Linux/Android case-sensitive filesystems. | Resolve case-insensitively in the host `NavigatorBackend` (or normalize filenames at build time). Trivial. |
| **CFX/GFX container** | `hudmenu.gfx` uses the CFX signature (zlib body, same 8-byte header as CWS) | **Works** — Ruffle accepts it directly; no re-headering needed. | None. |
| **SetBackgroundColor / vectors / gradients / embedded fonts** | Everywhere | **Works** — proven by the renders (stage bg 51,51,51, vector Bethesda logo, gradient level meter, real Futura text). | None. |

**What was NOT found:** no `fscommand`, no `LocalConnection`, no `NetConnection`, no video, no `SharedObject` use in the menus, no `Stage` fullscreen requests. The menus are pure UI: vectors, text, bitmaps, tweens, and the ExternalInterface bridge.

---

## 5. Implementation plan

Ordered; each milestone ends with a verifiable artifact. The Bevy UI stays in place until its Ruffle replacement reaches parity per screen — no big-bang cutover.

### M1 — Harness: hudmenu renders a frame from stub game data (first milestone)
- New standalone crate/bin (outside the engine's hot path): Ruffle as a git dep (pin 0.7.1 tag `89a7049…` or a tested nightly), `WgpuRenderBackend<TextureTarget>::for_offscreen` (own device — no Bevy coupling yet), `PlayerBuilder` with a stub `ExternalInterfaceProvider` (logs calls, answers `GetButtonFromUserEvent`/`RequestIsOnPC`-style queries with canned values) and a stub `NavigatorBackend` serving the synthesized `gfxfontlib.swf` from memory.
- Load `hudmenu.gfx`, `run_frame()` ×N, drive it with stub data via `call_internal_interface("call", …)`: set health/magicka/stamina percents, compass angle, location name, crosshair target. Screenshot via `capture_frame()`; compare against `shot-hud-fontlib.png` + a meters-visible frame.
- **Verifies:** the whole host loop (build → load → drive → render → capture) without touching the engine. **Unknowns flagged:** Ruffle build time/size on this box (measure); AVM1 CPU cost per frame (measure); whether `for_offscreen` works under lavapipe (it should — same wgpu path as the desktop binary, which ran here).
- **Artifact:** harness + a meters-visible HUD PNG.

### M2 — Fontlib + translation, production-shaped

> **Note (2026-11-16):** M2 as executed became the meter/location
> investigation instead — the M1 harness rendered the HUD but the
> meters and location name never appeared. That is now SOLVED (three
> root causes: missing `Stage.visibleRect`/`safeRect` → `0003`;
> 0–1 vs 0–100 percent scale in the drive; `gotoAndStop` ignoring
> float frame numbers → `0004`; see `crates/ruffle-harness/README.md`).
> The fontlib-production + translation work below is still open — it
> folds into M3 or the M5 menu work.

- Move the `gfxfontlib.swf` synthesis into the asset pipeline (build-time, from the player's own `fonts_en.swf` + `fontconfig.txt`) or into the `NavigatorBackend` (serve from memory). Decide by M1's measurements; either is fine.
- `$` translation: implement the Ruffle translator-hook patch in the fork (preferred) or the build-time string repack. Verify: pause menu shows "QUESTS / GENERAL STATS / SYSTEM", dialogue shows "Exit".
- **Artifact:** pause + dialogue screenshots with real English text.

### M3 — Resolve the nested-import blocker (the fork decision)

> **DONE 2026-11-16 — Option A landed. The gate PASSED.** The
> inventory menu renders in the harness with all three components
> (ItemCard, InventoryLists, BottomBar) in real fonts, driven with
> stub data: 10 categories, 5 items, the Iron Dagger card
> (DAMAGE 4 / WEIGHT 2.5 / VALUE 10), bottom bar (Damage 4,
> Carry Weight 45/300, Gold 127, red health bar). Proof:
> `screenshots/ruffle-m3-inventory2/ruffle-m1-hud.png` (viewed;
> 446,209 non-background pixels vs 0 for the blank baseline).
> The fix is `0005-nested-imports.patch` in the vendored Ruffle:
> `load_asset_movie` preloads imported movies to completion,
> resuming after each nested import (hand-rolled yield, no new
> dep), and a failed import now un-wedges its importer;
> `import_exports_of_importer` runs only at preload completion
> (it used to occupy IDs the clip's own later definitions need —
> 65 lost shapes); imported assets win their ID via
> `register_character_overwrite`. All three components preload
> `true`, zero collisions, zero non-registered IDs. The HUD render
> is pixel-identical to the M2 proof (sha256 `c21cbe7d…`), so the
> shared loader paths did not regress. Variable injection also
> landed in M3 (`0006-set-clip-variable.patch` — Scaleform's
> `SetVariable`; the `entryList` injection is what populates the
> items list). The SWF surgery option (B) was NOT implemented —
> unnecessary. Tag 1009 was NOT done in M3 (keycap icons still
> render as green/blue blocks in the driven inventory screenshot);
> it stays open for M5. Full detail:
> `crates/ruffle-harness/README.md` (M3 status) and
> `vendor/ruffle/README-DOVAHENGINE.md` (0005/0006).

- **Option A (preferred if it lands cleanly):** fix #22426 in the Ruffle fork — recursive preload + per-movie ID namespacing. The issue has a minimal in-repo repro (`tests/tests/swfs/import_assets/avm1_imports_avm1` + `repro.zip`). Verify against the inventory menu: lists, item card, and bottom bar appear.
- **Option B:** build-time SWF surgery — merge the three component SWFs into inventory/magic/container with remapped IDs. Verify the same way.
- Whichever wins, also implement tag 1009 (or the repack equivalent) in the same milestone — the keycap icons are part of "the real interface."
- **Artifact:** inventory menu screenshot with lists + item card + bottom bar + keycap icons. **This is the go/no-go gate for the whole path**: if neither option works, Ruffle cannot do the inventory family and the verdict flips to partial (HUD/dialogue/pause/start via Ruffle, inventory via the extraction fallback, §6).

### M4 — HUD live in the engine — DONE 2026-11-16 (Cortana), verified by Jarvis
- **As executed:** the Ruffle player lives in the RENDER world (a `NonSend` resource, built lazily by a render system in `RenderSystems::PrepareAssets` — the main world cannot see Bevy's asynchronously-created device, and `FutureRenderResources` is `pub(crate)`). It shares Bevy's wgpu device (`Descriptors` from `RenderDevice`/`RenderQueue`/`RenderAdapter`/`RenderInstance`; Cargo.lock has exactly one wgpu, 30.0.2). Compositing is option (b), CPU readback: `capture_frame()` → mailbox (`Arc<Mutex<Option<RgbaImage>>>`) → a Bevy `Image` (`Rgba8Unorm`, NOT sRGB — Ruffle renders in sRGB space already) on a fullscreen `ImageNode` with `Pickable { should_block_lower: false, is_hoverable: false }` so taps pass through. Option (a) (GPU composite) is deferred to the M6 performance pass. The movie steps at its own 30 fps via a time accumulator (max 3 steps/render frame); `run_frame` sets `needs_render` unconditionally, so idle-gating must skip `run_frame` itself (M6).
- **The extract bug (the one real M4 defect):** the first build's extract system used plain `Res<T>` — which reads the RENDER world, not the main world — so the render system never saw the assets (no error, no player, no HUD; the inn shot was HUD-less). The fix is `Extract<Res<T>>` (`bevy::render::Extract`, bevy_extract's `MainWorld` swap). Documented on `extract_ruffle_hud`.
- **Bridge (all live):** health/magicka 100.0 (no combat/magic systems — same hardcode as the save system and `update_bars`); stamina from the `Stamina` resource; compass `SetCompassAngle(heading, heading, true)` every frame (`heading_degrees` of the player's yaw — the compass fix's math); location `SetLocationName` on change (interior CELL FULL via plugin + strings, exterior hardcoded "Riverwood"); crosshair `SetCrosshairTarget` on change in USE priority (door > NPC > pickup > container) — load doors show the DESTINATION (interior CELL FULL via the `SceneTarget`'s new `form_id`, exterior "Riverwood"; the DOOR base record's FULL is a generic "Wooden Door", proven by the first proof shot), room doors show the door record's FULL; hints: the channel is wired, no hint triggers exist yet (M4 drives none — the inn-door prompt is the crosshair).
- **Fontlib at runtime:** `read_file_latest` (Patch.bsa wins — its hudmenu.gfx is the study's copy, md5 c6a1f287; its fonts_en.swf has 16 fonts at shifted IDs) + `synthesize_fontlib` in the harness crate (byte-identical to the study's fontlib from the Interface fonts; the Patch fontlib renders the HUD byte-identically to the M2 proof, sha256 c21cbe7d…). The engine synthesizes from the game's data path, never a dev path.
- **`safeRect` insets:** patch `0007-safe-area-insets.patch` adds `Player::set_safe_area_insets(l, t, r, b)` (pixels → twips); the engine wires it from `RuffleHudAssets.safe_insets` (0,0,0,0 on desktop; the phone's real insets via the Android safe-area API are future work).
- **Flag + fallback:** `--ruffle-hud` CLI flag (screenshot path, overrides the setting unconditionally) + `GameSettings.ruffle_hud` (default true, third Gameplay row "Real HUD" in the pause settings; the checkbox render/click/keyboard paths were hardcoded to `invert_y` for all rows — now index-based helpers). The Bevy HUD stays: its display nodes carry a `BevyHudDisplay` marker and hide only when the Ruffle HUD is enabled AND has delivered a frame; if the player fails to build, the Bevy HUD stays. The Ruffle HUD node is created lazily on the first captured frame.
- **Proof (all viewed):** `screenshots/ruffle-m4-inn/ruffle-hud-inn-early.png` (30 frames: compass "E", "Sleeping Giant Inn", crosshair + "Wooden Door" — the pre-fix shot), `screenshots/ruffle-m4-inn/ruffle-hud-inn-fixed.png` (30 frames, after the door-name fix: the crosshair still reads "Wooden Door" — the nearest door to the default inn viewpoint is a ROOM door, and that IS the real game's prompt for room doors), `screenshots/ruffle-m4-ext/ruffle-hud-ext-fixed.png` (30 frames, after the fix: the crosshair reads "Sleeping Giant Inn" — the exterior LOAD door resolves through its `DoorTrigger` to the destination CELL's FULL, proving the fix end to end), `screenshots/ruffle-m4-inn/ruffle-hud-inn.png` (75 frames: bars + compass + crosshair; the location has faded out by then — vanilla behavior, the 170-frame location timeline), `screenshots/ruffle-m4-ext/ruffle-hud-ext.png` (Riverwood, compass "W", "Riverwood", bars, crosshair "Wooden Door" — pre-fix), `screenshots/ruffle-m4-bevy/bevy-hud-inn.png` (Bevy HUD, same scene: compass "E", same fills 100/40/100 — the two HUDs agree). Stamina 40 driven by `--screenshot-stamina 40` in all Ruffle shots.
- **Also in M4:** `SceneTarget::Interior` now carries `form_id` (the door-destination lookup needs it; `--cell` uses 0 = unknown); `AssetProvider::read_file_latest` + `data_dir()`; B0001 fix in the visibility system (two `&mut Visibility` queries need `Without` filters to prove disjointness — Bevy validates at system init even for run_if-gated systems).
- **Deliberately left out:** compass markers (no quest/map marker feed yet — the strip shows letters only); subtitles/messages; shout meter; hint triggers; GPU compositing (M6); phone safe-area insets (API wired, values zero); tag-1009 keycap art + `$` tokens (still literal — M5); the 3D item model (`UpdateItem3D` — M5). No APK in M4 (desktop verification first).
- **Tests:** dovah-app lib 227/227 (11 ruffle tests), ruffle-harness 18/18, clippy 0, fmt clean.

### M5 — Menus, one at a time
Order by value and risk: dialogue → pause (journal) → inventory → container → magic → favorites → stats → map → start (main menu) → loading.
- Each menu: implement its SWF→game methods in the `ExternalInterfaceProvider` dispatch (reusing the engine systems the Bevy UI already calls — the Bevy UI is the spec for what each method must do), inject its list data (`entryList`, `CompassTargetDataA`), serve its `img://` images (map render, save screenshots).
- Input: full touch→mouse + on-screen-button→key routing; verify list scrolling, selection, and the item-3D drag-rotate (`StartMouseRotation`/`StopMouseRotation`/`ZoomItemModel`/`UpdateItem3D` — the 3D item view is game-rendered, composited by the SWF's `ItemCard`).
- **Artifact per menu:** phone screenshots + a checklist of its callbacks exercised.

### M6 — Cutover + cleanup
- Remove the Bevy UI screens as their Ruffle replacements reach parity (keep the Bevy HUD as a fallback until M4 is proven on the phone). Delete the duplicated layout code; keep the engine systems (inventory, dialogue, quests) — they now feed Ruffle instead of Bevy UI.
- Performance pass: only `run_frame`/`render` when `needs_render` or the movie's frame rate demands it; measure on the phone.
- **Artifact:** v0.5.0 (or next) APK with the full real interface; phone proof list per menu.

### Honest unknowns (flagged, not hidden)
1. **Texture sharing with Bevy** (M4 option (a)) is unproven — the fallback (CPU readback) is proven, so this is a performance question, not a feasibility one.
2. **Ruffle build time/size on the 6 GB box and in the APK** — unmeasured. ruffle_core is a large crate (AVM1 + AVM2 + render); the APK will grow. Measure at M1.
3. **AVM1 interpreter cost on the phone CPU** — the menus are light, but unmeasured. The HUD's per-frame work is a handful of number updates + tweens.
4. **The #22426 fix (option A)** — RESOLVED 2026-11-16 (M3): the fix
   landed in the vendored Ruffle as `0005-nested-imports.patch`
   (resume-loop preload + deferred export copy + import-wins ID
   registration). The inventory menu renders; the HUD is
   pixel-identical to the M2 proof. Worth proposing upstream.
5. **Variable injection** — RESOLVED 2026-11-16 (M3): `Player::
   set_clip_variable` (patch 0006) sets `entryList` arrays (and
   any value) on clips by dotted path; the inventory drive proves
   it end to end. The `mutate_with_update_context` escape hatch
   was NOT needed — a public patch method was cleaner.
6. **The `swf` crate's tag-1009 parser** — Ruffle's `swf` crate warns on unknown tags rather than failing, so implementing 1009 means adding a tag definition + a render path; the effort is a few hundred lines, unmeasured.
7. **Per-call argument shapes** for the ~80 SWF→game methods (§2.3) — enumerated, not all read. Mechanical grep work during M5, but a method's args could surprise (e.g. `UpdateItem3D`'s model-passing convention).

---

## 6. Fallback: asset extraction (brief, per the redirect)

> **Note (2026-11-16):** the M3 gate PASSED, so this fallback is
> not the plan. It stays documented in case a later menu proves
> unworkable in Ruffle.

If Ruffle proves unworkable at the M3 gate, the fallback is ffdec extraction: JPEXS ffdec 26.5.0 (already downloaded, `interface-extract/tools/ffdec/`, CLI works) exports vector shapes (SVG), bitmaps (PNG), and fonts (TTF) from the `.swf` files into `interface-extract/flash-exports/`. Bevy could then rebuild the HUD pixel-faithfully from the real art (shapes as textures, the real Futura fonts as font assets — which also closes the font gap). What would be lost vs running the SWF: all ActionScript logic (meter auto-hide timing, compass marker math, list filtering, tween animations, input handling) would have to be re-implemented in Rust — which is exactly the approximation work Dylan rejected, just with better art. The extraction path is therefore a fallback for *art*, not a substitute for *behaviour*. It was not evaluated further per the redirect.

---

## Appendix — evidence index

**Tools** (in `interface-extract/tools/`): Ruffle 0.7.1-stable (`ruffle-0.7.1-linux-x86_64.tar.gz`, 13.4 MB, sha256 `67950dfd…fdd99`); Ruffle nightly 0.8.0.2026.11.16 (`nightly/ruffle`, 13.3 MB); ffdec 26.5.0 (`ffdec_26.5.0.zip`, 22.3 MB, sha256 `8d00c271…66d33`); `libxkbcommon-x11-0` installed via apt (Ruffle/winit aborts without it). Harness: `tools/ruffle-test/run_test.sh` (Xvfb:99, `--no-gui --graphics vulkan --width 1280 --height 720 --frame-rate 30`, ffmpeg x11grab at 7s; sleep 3s between runs — Xvfb:99 races).

**Key files:** synthesized fontlib `tools/ruffle-test/gfxfontlib.swf` (487,761 B, md5 `445919118b3357bf040d69a760edd961`); test dir `tools/ruffle-test/hud-test/` (menu + fontlib); script exports `interface-extract/flash-exports/scripts/` (2,150 `.as` files); screenshots `tools/ruffle-test/shot-*-fontlib.png` (md5s in §1).

**Sources:** Ruffle master `core/src/player.rs` (PlayerBuilder L4846, `run_frame` L3757, `render` L3828, `call_internal_interface` L4640, `set_external_interface_provider` L4623, `handle_event` L1957, `set_window_mode` L1900, `mutate_with_update_context` L4194), `core/src/external.rs` (Value L43, ExternalInterfaceProvider L674), `core/src/backend/navigator.rs` (NavigatorBackend L491), `core/src/avm1/globals/stage.rs` (Stage properties), `render/wgpu/src/backend.rs` (`WgpuRenderBackend::new` L430, `for_offscreen` L350, `capture_frame` L395), `render/wgpu/src/target.rs` (RenderTarget L25, TextureTarget L354), workspace `Cargo.toml` (wgpu 30.0.1, license MIT OR Apache-2.0, edition 2024). Issue: ruffle-rs/ruffle#22426 (opened 2025-12-06). Ruffle 0.7.1 blog: "supports AS1/2/3 pretty well, but it's still not finished."

**What this study did not do:** build Ruffle from source; run any cargo command; edit engine code; test on the phone; test magicmenu/container screenshots individually; read every SWF→game call site's arguments (§2.3); verify Bevy 0.20's exact `RenderInstance`/`RenderDevice` resource names (stated as "verify at implementation time").
