//! Dovahengine renderer (M6): load a Skyrim cell and walk in it.
//!
//! Usage:
//!
//! ```text
//! dovah-app [--esm PATH] [--data DIR] [--cell EDID | --exterior GX GY]
//!           [--screenshot PATH] [--frames N] [--size WxH]
//!           [--screenshot-ui] [--screenshot-stamina N]
//!           [--camera x,y,z:lx,ly,lz] [--door-open HEXFORMID] [--scripts]
//!           [--sawmill-time SECS] [--inventory] [--give HEXFORMID,...]
//! ```
//!
//! Without `--screenshot` it opens a window: WASD to move, mouse to
//! look, Space to jump, Shift to sprint, E to use the nearest door,
//! Esc to release the mouse.
//! The ESM parse runs on a worker thread behind a loading screen
//! (M8.1) instead of blocking startup. With `--screenshot` it runs
//! headless, renders the cell to an image target, saves one frame
//! as a PNG, and exits — that is how the milestone is verified on
//! a machine with no display. `--screenshot-ui` draws the touch
//! HUD (bars, compass, buttons) into the screenshot too, so
//! desktop renders can prove HUD work the phone will show; the
//! touch plugin needs the Stamina resource the player plugin
//! normally owns, so the flag inserts it, at the value of
//! `--screenshot-stamina` (0–100, full by default) — the way to
//! prove a drained bar without a sprinting thumb. `--camera`
//! pins the screenshot viewpoint (position + look-at in Bevy
//! metres) — the way to take close-ups of one NPC, door, or
//! corner instead of the default room view. `--door-open`
//! (M15) takes a door ref's hex form ID and renders that door
//! already swung open — the proof shot for the swing.
//! `--inventory` (M17) opens the inventory in the screenshot —
//! the proof shot for the bag screen and the selected item's
//! 3D model — and `--give` seeds the inventory with items by
//! base form ID (comma-separated hex, e.g. `12eb7,12e49`), so
//! the shot can show equipped gear without playing the game.
//! `--inventory-tab` (redesign) opens the shot on one category
//! instead of All. `--pause` (UI milestone 1) opens the pause
//! screen in the screenshot — the proof shot for the quests /
//! stats / system menu — and `--pause-tab NAME` opens it on
//! one tab or System category instead of Quests (quests,
//! stats, system, settings, controls, load, save, help, quit,
//! gameplay, display, audio).
//!
//! With `--scripts` it does not open a window at all: it prints the
//! M7 script report for the cell — which Papyrus scripts its
//! records attach, which `.pex` definitions the data folder has,
//! and which scripts would run — then exits.
//!
//! Defaults point at the workspace's `test-data/` folder (Dylan's
//! own Skyrim files, read at runtime, never copied into the repo)
//! and the Sleeping Giant Inn, the M6 acceptance test.
//!
//! M8: this is the crate's library target (`cdylib` for the Android
//! APK — `cargo-apk` cannot build a bare binary). The desktop binary
//! in `main.rs` is a thin wrapper that calls [`main`]. On Android
//! the `#[bevy_main]` macro on [`main`] generates the
//! `android_main` entry point, the ESM is found on the phone by
//! [`android::find_esm`] instead of CLI flags, and touch controls
//! come from the [`touch`] module.

mod android;
/// Melee combat (fists + weapons): health from the vanilla
/// formula, hit detection, damage, death. Private — the
/// plugin + systems are wired in `lib.rs`.
mod archery;
pub mod assets;
/// Public so examples (diagnostics) can use the REAL
/// cell-loading + terrain-bake path instead of a copy that
/// drifts (terrain_probe).
pub mod cell;
/// Public so examples (diagnostics) can use the REAL
/// character-menu index builders instead of a copy that drifts.
pub mod character;
/// Public so examples (diagnostics) can use the REAL
/// collision conversion + obstacle grid instead of a copy that
/// drifts (collision_walk).
pub mod collision;
mod combat;
/// Public so examples (diagnostics) can use the REAL mesh/texture
/// conversion path instead of a copy that drifts (pine_light_test).
pub mod convert;
pub mod coords;
mod dialogue;
pub mod doors;
mod interact;
mod inventory;
mod item_model;
mod land;
/// Mill wheel turning: the waterwheel rotates about its own
/// axle from its NIF `Idle` sequence (private — the spawn path
/// runs through `scene`, the system is registered by the player
/// + screenshot plugins).
mod mill;
mod npc;
mod pause;
/// Frame-time telemetry for the phone log (v0.4.11 perf work):
/// the `FrameStats` accumulator + `log_frame_time` system
/// (private — registered by `ruffle::RuffleHudPlugin`, which is
/// added on both app paths).
mod perf;
mod player;
mod player_model;
mod ruffle;
mod save;
/// Sawmill blade: the blade spins through its authored
/// keyframed performance from its NIF's direct
/// `NiTransformController` (private — same shape as `mill`).
mod sawmill;
pub mod scene;
mod screenshot;
mod scripts;
mod startup_log;
mod touch;
/// Public so examples (diagnostics) can use the REAL
/// terrain-bake path (terrain_probe).
pub mod vtxt;
/// River water: flat per-cell planes + the UV flow system
/// (private — the spawn path runs through `scene`, the
/// system is registered by the player + screenshot plugins).
mod water;

use assets::AssetProvider;
use bevy::app::{App, PluginGroup, ScheduleRunnerPlugin, Startup, Update};
use bevy::asset::{AssetServer, Assets};
use bevy::color::Color;
use bevy::ecs::prelude::*;
use bevy::image::Image;
use bevy::light::GlobalAmbientLight;
use bevy::math::Vec3;
use bevy::mesh::Mesh;
use bevy::pbr::StandardMaterial;
use bevy::prelude::{in_state, NextState, OnEnter, States};
use bevy::render::RenderPlugin;
use bevy::state::app::AppExtStates;
use bevy::text::{FontSize, TextColor, TextFont};
use bevy::transform::components::Transform;
use bevy::ui::widget::{ImageNode, Text};
use bevy::ui::{AlignItems, BackgroundColor, FlexDirection, JustifyContent, Node, Val};
use bevy::window::{ExitCondition, WindowPlugin};
use bevy::winit::WinitPlugin;
use cell::{SceneTarget, SpawnPoint};
use scene::{Player, RoomBounds, SceneEntity, SceneSummary};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// M8.1: the windowed app starts on a loading screen while a
/// worker thread parses the ESM (~10 s on the phone); the scene
/// spawns when the data arrives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, States)]
enum AppState {
    #[default]
    Loading,
    /// The chunked scene build (the ANR fix): the loading screen
    /// stays alive with progress while `spawn_chunk` drains the
    /// `SpawnProgress` a budget at a time.
    Spawning,
    InGame,
}

/// Everything one scene load produces, handed to the app.
struct LoadedData {
    cell_data: cell::CellData,
    bounds: RoomBounds,
}

/// What the worker thread sends back. The FIRST load also delivers
/// the parsed plugin and the asset provider; both are kept as
/// resources afterwards — door transitions (M9) load the next
/// scene from the same parsed plugin instead of re-parsing the
/// 238 MB ESM, and scene building keeps using the same provider
/// (the worker thread never touches it).
enum LoadResult {
    First {
        plugin: Arc<dovah_esm::Plugin>,
        provider: AssetProvider,
        // M18: the localized strings (from Patch.bsa), loaded
        // once with the plugin — dialogue text and real names.
        strings: dialogue::GameStrings,
        data: LoadedData,
    },
    Scene {
        data: LoadedData,
    },
}

/// The parsed Skyrim.esm, kept for door transitions (M9). Shared
/// via Arc: the worker thread clones it, the app keeps its copy.
/// `pub(crate)` — the Spawning MapBake phase reads it for the
/// world-map bake walk.
#[derive(Resource)]
pub(crate) struct SharedPlugin(pub(crate) Arc<dovah_esm::Plugin>);

/// Channel end the loading system polls. The receiver sits behind
/// a Mutex because `std::sync::mpsc::Receiver` is `Send` but not
/// `Sync`, and Bevy resources must be both. The lock is held only
/// for the `try_recv` call.
#[derive(Resource)]
struct LoadReceiver(std::sync::Mutex<std::sync::mpsc::Receiver<Result<LoadResult, String>>>);

impl LoadReceiver {
    fn try_recv(&self) -> Result<Result<LoadResult, String>, std::sync::mpsc::TryRecvError> {
        // A poisoned lock (a previous holder panicked) must not
        // panic the loading system — recover the guard and carry
        // on. `try_recv` itself never panics, so in practice this
        // never triggers; it is belt-and-braces for the startup
        // path, where a panic aborts the whole app (v0.2.1).
        let guard = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.try_recv()
    }
}

/// Marker for the loading-screen UI root (despawned on arrival).
#[derive(Component)]
struct LoadingScreen;

/// Marker for the loading-screen title text (replaced on error).
#[derive(Component)]
struct LoadingTitle;

/// Marker for the loading-screen subtext (replaced on error).
#[derive(Component)]
struct LoadingText;

/// Marker for the loading-screen progress line (the chunked
/// build's counts, updated by `update_loading_progress`).
#[derive(Component)]
struct LoadingProgress;

struct Args {
    esm: PathBuf,
    data: PathBuf,
    /// The scene to load first: an interior cell by EDID
    /// (`--cell`), or a Riverwood exterior block (`--exterior GX
    /// GY`, M9). Defaults to the Sleeping Giant Inn.
    target: SceneTarget,
    screenshot: Option<String>,
    frames: u32,
    width: u32,
    height: u32,
    /// Draw the touch HUD into a screenshot (M11 proof hook).
    screenshot_ui: bool,
    /// Stamina value (0–100) for `--screenshot-ui` shots.
    screenshot_stamina: f32,
    /// Draw the real Skyrim HUD (Ruffle) into a screenshot (M4
    /// proof hook). Forces the Ruffle HUD on for the run,
    /// overriding the `ruffle_hud` setting; without it the
    /// screenshot path shows the Bevy HUD.
    ruffle_hud: bool,
    /// Camera override for screenshots (M12 proof hook): (position,
    /// look-at) in Bevy metres, from `--camera x,y,z:lx,ly,lz`.
    /// `None` = the default viewpoint.
    camera: Option<(Vec3, Vec3)>,
    /// Force one door open in screenshots (M15 proof hook): a ref
    /// form ID in hex, e.g. `13419`. The door spawns already swung
    /// open, via `DoorMemory` seeding — the same path the game
    /// uses for a door the player left open.
    door_open: Option<u32>,
    /// Pin the sawmill blade's performance time in screenshots
    /// (sawmill proof hook): seconds into the blade's 100.73 s
    /// performance, e.g. `35`. The blade sits still for the
    /// first 15.37 s, so without this a short render can never
    /// show it mid-spin.
    sawmill_time: Option<f32>,
    /// Open the inventory in the screenshot (M17 proof hook).
    inventory: bool,
    /// Item base form IDs (hex) to seed the inventory with (M17
    /// proof hook): `--give 12eb7,12e49`.
    give: Vec<u32>,
    /// Item base form IDs (hex) to give and then DROP on the
    /// first InGame frame (drop-diagnosis proof hook):
    /// `--drop-test 12eb7`. The drops run through the real
    /// `drop_item` path, so the screenshot shows where dropped
    /// items actually land.
    drop_test: Vec<u32>,
    /// Melee proof hook: `--melee-test N` teleports the player
    /// in front of Orgnar and queues N scripted attacks
    /// through the real attack system (`None` = no seed).
    melee_test: Option<u32>,
    /// Archery proof hook: `--archery-test N` teleports the
    /// player 8 m from Orgnar, grants a HuntingBow + N
    /// IronArrows, equips the bow, and queues N scripted
    /// shots through the real fire system (`None` = no seed).
    archery_test: Option<u32>,
    /// `--archery-speed MPS` overrides the PROJ flight speed
    /// for the proof shot (a slow arrow is visible across
    /// frames; the real 51.4 m/s crosses the inn in one
    /// lavapipe frame). `None` = the PROJ speed.
    archery_speed: Option<f32>,
    /// Which inventory tab to open on in the screenshot
    /// (inventory-redesign proof hook): `--inventory-tab
    /// weapons`. `None` = All (the default).
    inventory_tab: Option<inventory::InventoryTab>,
    /// Open dialogue with Delphine in the screenshot (M18 proof
    /// hook), with the first topic selected.
    dialogue: bool,
    /// Open the pause screen in the screenshot (UI milestone 1
    /// proof hook).
    pause: bool,
    /// Open the character menu in the screenshot (UI milestone
    /// 2 proof hook).
    character_menu: bool,
    /// Which character tab to open on in the screenshot (UI
    /// milestone 2 proof hook): `--character-tab magic`. `None`
    /// = Map (the default).
    character_tab: Option<character::CharacterSeed>,
    /// Skip loading normal maps (`--no-normal-maps`): a
    /// desktop-proof aid, NOT a quality setting. Normal maps
    /// roughly double texture memory (294 textures in the
    /// Riverwood exterior vs 115 without), and lavapipe keeps
    /// its own copy on top — the exterior render OOMs at 6 GB
    /// with them on. The phone (12 GB, real GPU) never sets
    /// this. The inn floor proof runs with normals ON.
    no_normal_maps: bool,
    /// Which pause tab / System category to open on in the
    /// screenshot (UI milestone 1 proof hook): `--pause-tab
    /// settings`. `None` = Quests (the default).
    pause_tab: Option<pause::PauseSeed>,
    /// Draw the loading screen (with the DovahEngine logo) into a
    /// screenshot (M15 show-build proof hook). The loading UI
    /// covers the scene; the capture proves the logo renders.
    screenshot_loading: bool,
    /// Write a save on the first InGame frame (save-system
    /// proof hook): `--save-test`. The file lands in the save
    /// dir for the caller to read back.
    save_test: bool,
    /// Load the named save file (a file name inside the save
    /// dir) on the first InGame frame (save-system proof
    /// hook): `--load-test save-<unix>.json`.
    load_test: Option<String>,
    scripts: bool,
}

/// Parse a comma-separated list of hex form IDs
/// (`12eb7,12e49`), with or without `0x` prefixes. Pure —
/// unit-tested.
fn parse_hex_list(raw: &str) -> Result<Vec<u32>, String> {
    raw.split(',')
        .map(|part| {
            let hex = part.trim().strip_prefix("0x").unwrap_or(part.trim());
            u32::from_str_radix(hex, 16)
                .map_err(|_| "--give needs hex form IDs, e.g. 12eb7,12e49".to_string())
        })
        .collect()
}

fn default_test_data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("test-data")
}

fn parse_args() -> Result<Args, String> {
    let test_data = default_test_data();
    let mut args = Args {
        esm: test_data.join("Skyrim.esm"),
        data: test_data,
        target: SceneTarget::Interior {
            form_id: 0x000133c6,
            edid: "RiverwoodSleepingGiantInn".to_string(),
        },
        screenshot: None,
        frames: 30,
        width: 1280,
        height: 720,
        screenshot_ui: false,
        screenshot_stamina: player::STAMINA_MAX,
        ruffle_hud: false,
        camera: None,
        door_open: None,
        sawmill_time: None,
        inventory: false,
        give: Vec::new(),
        drop_test: Vec::new(),
        melee_test: None,
        archery_test: None,
        archery_speed: None,
        inventory_tab: None,
        dialogue: false,
        pause: false,
        pause_tab: None,
        character_menu: false,
        character_tab: None,
        no_normal_maps: false,
        screenshot_loading: false,
        save_test: false,
        load_test: None,
        scripts: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let value = |it: &mut std::iter::Skip<std::env::Args>, flag: &str| {
            it.next().ok_or_else(|| format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "--esm" => args.esm = PathBuf::from(value(&mut it, "--esm")?),
            "--data" => args.data = PathBuf::from(value(&mut it, "--data")?),
            "--cell" => {
                args.target = SceneTarget::Interior {
                    // Unknown until the plugin loads; the load
                    // path matches by EDID.
                    form_id: 0,
                    edid: value(&mut it, "--cell")?,
                }
            }
            "--exterior" => {
                let gx: i32 = value(&mut it, "--exterior")?
                    .parse()
                    .map_err(|_| "--exterior needs grid X as a number".to_string())?;
                let gy: i32 = value(&mut it, "--exterior")?
                    .parse()
                    .map_err(|_| "--exterior needs grid Y as a number".to_string())?;
                args.target = SceneTarget::Exterior { grid: (gx, gy) };
            }
            "--screenshot" => args.screenshot = Some(value(&mut it, "--screenshot")?),
            "--no-normal-maps" => args.no_normal_maps = true,
            "--screenshot-ui" => args.screenshot_ui = true,
            "--ruffle-hud" => args.ruffle_hud = true,
            "--screenshot-stamina" => {
                args.screenshot_stamina = value(&mut it, "--screenshot-stamina")?
                    .parse()
                    .map_err(|_| "--screenshot-stamina needs a number 0-100".to_string())?
            }
            "--camera" => {
                let raw = value(&mut it, "--camera")?;
                let (pos, look) = raw
                    .split_once(':')
                    .ok_or("--camera needs the form x,y,z:lx,ly,lz".to_string())?;
                let parse_vec = |s: &str| -> Result<Vec3, String> {
                    let parts: Vec<&str> = s.split(',').collect();
                    if parts.len() != 3 {
                        return Err("--camera needs the form x,y,z:lx,ly,lz".to_string());
                    }
                    let nums: Vec<f32> = parts
                        .iter()
                        .map(|p| {
                            p.parse()
                                .map_err(|_| "--camera values must be numbers".to_string())
                        })
                        .collect::<Result<Vec<f32>, String>>()?;
                    Ok(Vec3::new(nums[0], nums[1], nums[2]))
                };
                args.camera = Some((parse_vec(pos)?, parse_vec(look)?));
            }
            "--scripts" => args.scripts = true,
            "--door-open" => {
                let raw = value(&mut it, "--door-open")?;
                let hex = raw.strip_prefix("0x").unwrap_or(&raw);
                args.door_open = Some(
                    u32::from_str_radix(hex, 16)
                        .map_err(|_| "--door-open needs a hex form ID, e.g. 13419".to_string())?,
                );
            }
            "--sawmill-time" => {
                let raw = value(&mut it, "--sawmill-time")?;
                args.sawmill_time = Some(
                    raw.parse()
                        .map_err(|_| "--sawmill-time needs seconds, e.g. 35".to_string())?,
                );
            }
            "--inventory" => args.inventory = true,
            "--give" => {
                let raw = value(&mut it, "--give")?;
                args.give = parse_hex_list(&raw)?;
            }
            "--drop-test" => {
                let raw = value(&mut it, "--drop-test")?;
                args.drop_test = parse_hex_list(&raw)?;
            }
            "--melee-test" => {
                args.melee_test = Some(
                    value(&mut it, "--melee-test")?
                        .parse()
                        .map_err(|_| "--melee-test needs a number".to_string())?,
                );
            }
            "--archery-test" => {
                args.archery_test = Some(
                    value(&mut it, "--archery-test")?
                        .parse()
                        .map_err(|_| "--archery-test needs a number".to_string())?,
                );
            }
            "--archery-speed" => {
                args.archery_speed = Some(
                    value(&mut it, "--archery-speed")?
                        .parse()
                        .map_err(|_| "--archery-speed needs a number".to_string())?,
                );
            }
            "--inventory-tab" => {
                let raw = value(&mut it, "--inventory-tab")?;
                args.inventory_tab = Some(
                    inventory::InventoryTab::from_name(&raw).ok_or_else(|| {
                        "--inventory-tab needs one of: all, weapons, apparel, potions, food, ingredients, books, keys, misc, scrolls".to_string()
                    })?,
                );
            }
            "--dialogue" => args.dialogue = true,
            "--pause" => args.pause = true,
            "--pause-tab" => {
                let raw = value(&mut it, "--pause-tab")?;
                args.pause_tab = Some(
                    pause::PauseSeed::from_name(&raw).ok_or_else(|| {
                        "--pause-tab needs one of: quests, stats, system, settings, controls, load, save, help, quit, gameplay, display, audio".to_string()
                    })?,
                );
            }
            "--character-menu" => args.character_menu = true,
            "--character-tab" => {
                let raw = value(&mut it, "--character-tab")?;
                args.character_tab =
                    Some(character::CharacterSeed::from_name(&raw).ok_or_else(|| {
                        "--character-tab needs one of: map, magic, items, skills".to_string()
                    })?);
            }
            "--screenshot-loading" => args.screenshot_loading = true,
            "--save-test" => args.save_test = true,
            "--load-test" => args.load_test = Some(value(&mut it, "--load-test")?),
            "--frames" => {
                args.frames = value(&mut it, "--frames")?
                    .parse()
                    .map_err(|_| "--frames needs a number".to_string())?
            }
            "--size" => {
                let size = value(&mut it, "--size")?;
                let (w, h) = size
                    .split_once('x')
                    .ok_or("--size needs the form WxH, e.g. 1280x720".to_string())?;
                args.width = w
                    .parse()
                    .map_err(|_| "--size width is not a number".to_string())?;
                args.height = h
                    .parse()
                    .map_err(|_| "--size height is not a number".to_string())?;
            }
            "--help" | "-h" => {
                println!(
                    "dovah-app [--esm PATH] [--data DIR] [--cell EDID | --exterior GX GY] [--screenshot PATH] [--frames N] [--size WxH] [--screenshot-ui] [--screenshot-stamina N] [--ruffle-hud] [--camera x,y,z:lx,ly,lz] [--door-open HEXFORMID] [--sawmill-time SECS] [--scripts] [--inventory] [--give HEXFORMID,...] [--drop-test HEXFORMID,...] [--melee-test N] [--archery-test N] [--archery-speed MPS] [--inventory-tab NAME] [--dialogue] [--pause] [--pause-tab NAME] [--screenshot-loading] [--save-test] [--load-test FILE]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

fn run() -> Result<(), String> {
    // M8: mutated only by the Android branch below; allow the
    // unused-mut warning on desktop.
    #[allow(unused_mut)]
    let mut args = parse_args()?;
    // M8: on Android there are no CLI flags and the compile-time
    // test-data path does not exist on the phone. Find Skyrim.esm on
    // the device instead; if it is not there (or cannot be read),
    // run the message screen that tells Dylan exactly what to do.
    #[cfg(target_os = "android")]
    {
        startup_log::log("ESM search start");
        match android::find_esm() {
            android::EsmSearch::Found { esm, data_dir } => {
                startup_log::log(&format!(
                    "ESM search: found {} (data dir {})",
                    esm.display(),
                    data_dir.display()
                ));
                args.esm = esm;
                args.data = data_dir;
            }
            android::EsmSearch::NotFound { checked } => {
                startup_log::log(&format!(
                    "ESM search: not found (checked {} paths) — message app",
                    checked.len()
                ));
                android::run_message_app(android::MissingReason::NotFound { checked });
                return Ok(());
            }
            android::EsmSearch::ExistsButUnreadable { path } => {
                startup_log::log(&format!(
                    "ESM search: exists but unreadable {} — message app",
                    path.display()
                ));
                android::run_message_app(android::MissingReason::Unreadable { path });
                return Ok(());
            }
        }
    }
    if args.scripts {
        // M7 wiring: report the cell's scripts, no window.
        // Synchronous — no app, no loading screen. The report is
        // cell-based, so it needs an interior target.
        let SceneTarget::Interior { edid, .. } = &args.target else {
            return Err("--scripts needs an interior cell (--cell)".to_string());
        };
        println!("loading {} ...", args.esm.display());
        let plugin = dovah_esm::Plugin::open(&args.esm)
            .map_err(|e| format!("failed to parse {}: {e}", args.esm.display()))?;
        let mut provider = AssetProvider::new(&args.data);
        print_provider(&args.data, &provider);
        let report = scripts::build_report(&plugin, edid, &mut provider)
            .ok_or_else(|| format!("no CELL with EDID '{edid}'"))?;
        scripts::print_report(&report);
        return Ok(());
    }

    // v0.2.2: the main-app build is the step v0.2.0/v0.2.1 died
    // in (before the first frame), so it gets its own stage lines.
    // If the log stops here, the crash is in app construction.
    startup_log::log_rss("main app: building");
    let mut app = App::new();
    insert_shared_resources(&mut app);
    // UI milestone 1: saves + settings live in a subdir of the
    // data dir (the phone's Downloads on Android, test-data on
    // desktop) — never in the repo.
    app.insert_resource(save::SaveDir(args.data.join("dovahengine-saves")));

    match &args.screenshot {
        Some(path) => {
            // Headless: load synchronously (no loading screen —
            // nothing to look at), then no window,
            // ScheduleRunnerPlugin drives the loop, one frame is
            // captured to a PNG and the app exits.
            let LoadResult::First {
                plugin,
                provider,
                strings,
                data,
            } = load_first(&args.esm, &args.data, &args.target)?
            else {
                unreachable!("load_first always returns First");
            };
            app.insert_resource(data.cell_data)
                .insert_resource(provider)
                .insert_resource(strings)
                .insert_resource(data.bounds)
                .insert_resource(SharedPlugin(plugin));
            let config = screenshot::ScreenshotConfig::new(path.clone(), args.frames);
            app.insert_resource(scene::NormalMaps(!args.no_normal_maps));
            app.insert_resource(config)
                .add_plugins(
                    bevy::DefaultPlugins
                        .set(WindowPlugin {
                            primary_window: None,
                            exit_condition: ExitCondition::DontExit,
                            ..Default::default()
                        })
                        // Synchronous pipeline compilation: all
                        // shaders ready for the first frame, so the
                        // screenshot never catches a half-built
                        // pipeline (same as Bevy's headless example).
                        .set(RenderPlugin {
                            synchronous_pipeline_compilation: true,
                            ..Default::default()
                        })
                        .disable::<WinitPlugin>(),
                )
                // The state machine (Loading -> Spawning ->
                // InGame) is shared with the windowed path —
                // the screenshot runner pumps Spawning until the
                // chunked build finishes, then captures. ORDER
                // IS LOAD-BEARING (v0.2.3): `init_state` needs
                // the `StateTransition` schedule from
                // `StatesPlugin`, inside `DefaultPlugins` — so
                // the plugin set MUST be added first.
                .init_state::<AppState>()
                .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
                    1.0 / 30.0,
                )))
                .add_plugins(screenshot::ScreenshotPlugin)
                // M17: the inventory plugin (resources, doll
                // target, toggle/refresh systems) and the
                // interact plugin (pickupable/container indexes).
                // M18: the dialogue plugin (strings, index,
                // talkable NPCs, open/close systems). UI
                // milestone 1: the pause plugin (quests/stats/
                // system menu, settings, saves). Their Startup
                // systems run before the scene's OnEnter(InGame) chain —
                // plugins are added first.
                .add_plugins(inventory::InventoryPlugin)
                .add_plugins(interact::InteractPlugin)
                .add_plugins(dialogue::DialoguePlugin)
                .add_plugins(pause::PausePlugin)
                .add_plugins(character::CharacterPlugin)
                .add_plugins(save::SavePlugin)
                .add_plugins(combat::CombatPlugin)
                .add_plugins(archery::ArcheryPlugin)
                // M4: the real Skyrim HUD (Ruffle). The plugin is
                // always added; the `--ruffle-hud` override below
                // decides whether it draws on this path.
                .add_plugins(ruffle::RuffleHudPlugin);
            // M4: the screenshot path is driven by the CLI flag,
            // not the setting — insert the override unconditionally
            // (false without `--ruffle-hud`, so a saved
            // settings.json with `ruffle_hud: true` cannot leak
            // the Ruffle HUD into a Bevy-HUD comparison shot).
            app.insert_resource(ruffle::RuffleHudCliOverride(args.ruffle_hud));
            // M17: `--give` / `--inventory` seed the inventory
            // for the proof shot; the seed is applied by
            // `apply_inventory_seed` in the OnEnter(InGame) chain,
            // after the catalog is built. The redesign adds
            // `--inventory-tab` (which tab the shot opens on).
            if !args.give.is_empty() || args.inventory || args.inventory_tab.is_some() {
                app.insert_resource(inventory::InventorySeed {
                    items: args.give.clone(),
                    open_inventory: args.inventory,
                    tab: args.inventory_tab,
                });
            }
            // Drop diagnosis: `--drop-test` gives each listed
            // item and drops it on the first InGame frame, via
            // `apply_drop_test_seed` in the OnEnter(InGame)
            // chain, after the inventory seed.
            if !args.drop_test.is_empty() {
                app.insert_resource(DropTestSeed(args.drop_test.clone()));
            }
            // Melee: `--melee-test N` teleports the player in
            // front of Orgnar and queues N scripted attacks
            // through the real attack system; applied by
            // `apply_melee_test_seed` in the OnEnter(InGame)
            // chain, after `build_combatants`.
            if let Some(n) = args.melee_test {
                app.insert_resource(combat::MeleeTestSeed {
                    attacks: n,
                    npc_base: Some(0x0001_3479),
                });
            }
            // Archery: `--archery-test N` teleports the player
            // 8 m from Orgnar, grants a HuntingBow + N
            // IronArrows, equips the bow, and queues N scripted
            // shots through the real fire system; applied by
            // `apply_archery_test_seed` in the OnEnter(InGame)
            // chain, after `build_combatants`. `--archery-speed`
            // overrides the PROJ flight speed for the proof
            // shot (a slow arrow is visible across frames).
            if let Some(n) = args.archery_test {
                app.insert_resource(archery::ArcheryTestSeed {
                    shots: n,
                    npc_base: Some(0x0001_3479),
                });
            }
            app.insert_resource(archery::ArcherySpeedOverride(args.archery_speed));
            // M18: `--dialogue` opens dialogue with Delphine
            // (NPC_ 0x00013478) for the proof shot, first topic
            // selected; applied by `apply_dialogue_seed` in the
            // OnEnter(InGame) chain.
            if args.dialogue {
                app.insert_resource(dialogue::DialogueSeed {
                    npc_base: Some(0x0001_3478),
                    select_first_topic: true,
                });
            }
            // UI milestone 1: `--pause` / `--pause-tab` open the
            // pause screen for the proof shot, on the requested
            // tab / System category; applied by
            // `apply_pause_seed` in the OnEnter(InGame) chain.
            if args.pause || args.pause_tab.is_some() {
                let seed = args.pause_tab.clone().unwrap_or(pause::PauseSeed {
                    tab: pause::PauseTab::Quests,
                    system: pause::PauseSystem::Settings,
                    settings_options: None,
                });
                app.insert_resource(seed);
            }
            // UI milestone 2: `--character-menu` /
            // `--character-tab` open the character menu for the
            // proof shot, on the requested tab; applied by
            // `apply_character_seed` in the OnEnter(InGame)
            // chain.
            if args.character_menu || args.character_tab.is_some() {
                let seed = args
                    .character_tab
                    .clone()
                    .unwrap_or(character::CharacterSeed {
                        tab: character::CharacterTab::Map,
                    });
                app.insert_resource(seed);
            }
            // M15 show build: `--screenshot-loading` draws the
            // loading screen (with the DovahEngine logo) into
            // the shot; applied by
            // `spawn_screenshot_loading_screen` at the end of
            // the OnEnter(InGame) chain.
            if args.screenshot_loading {
                app.insert_resource(ScreenshotLoading);
            }
            // M11: `--screenshot-ui` draws the touch HUD into the
            // shot. The touch plugin needs the Stamina resource
            // the player plugin normally owns — the screenshot
            // path has no player plugin, so the flag inserts it
            // here, at the `--screenshot-stamina` value. M4: the
            // Ruffle HUD drive reads the same resource, so
            // `--ruffle-hud` inserts it too (without the touch
            // plugin — the Ruffle HUD hides the Bevy display).
            if args.screenshot_ui || args.ruffle_hud {
                app.insert_resource(player::Stamina {
                    value: args.screenshot_stamina,
                });
            }
            if args.screenshot_ui {
                app.add_plugins(touch::TouchControlsPlugin);
            }
            // M12: `--camera` pins the screenshot viewpoint
            // (close-ups of the NPCs, door checks, ...).
            if let Some((position, look_at)) = args.camera {
                app.insert_resource(scene::CameraOverride { position, look_at });
            }
            // M15: `--door-open` seeds DoorMemory so the door
            // spawns already swung open — the same path the game
            // uses for a door the player left open.
            if let Some(ref_form_id) = args.door_open {
                let mut memory = doors::DoorMemory::default();
                memory.open.insert(ref_form_id);
                app.insert_resource(memory);
            }
            // Sawmill: `--sawmill-time` pins the blade's
            // performance time for the render (the blade sits
            // still for the first 15.37 s of its performance).
            if let Some(secs) = args.sawmill_time {
                app.insert_resource(sawmill::SawmillTimeOverride(secs));
            }
            // Save system: `--save-test` writes a save on the
            // first InGame frame; `--load-test <file>` loads
            // the named save (a file name inside the save dir)
            // on the first InGame frame. Both are consumed by
            // the save plugin's Update systems.
            if args.save_test {
                app.insert_resource(save::SaveSeed);
            }
            if let Some(file) = args.load_test.clone() {
                app.insert_resource(save::LoadTestSeed { file });
            }
            // Create the render target NOW, on the App itself, so the
            // resource exists before `spawn_camera` runs (see the
            // note on `ScreenshotPlugin`).
            let target_image = screenshot::create_target_image(args.width, args.height);
            let handle = app
                .world_mut()
                .resource_mut::<Assets<Image>>()
                .add(target_image);
            app.insert_resource(screenshot::ScreenshotTarget { image: handle });
            // The screenshot path runs the SAME state machine as
            // the windowed path (one implementation — see
            // `add_scene_state_systems`): the data is already
            // loaded synchronously above, so Startup only kicks
            // off Spawning. The chunk system builds the scene a
            // budget at a time, the OnEnter(InGame) chain
            // (camera, catalog, interactables, dialogue, quest
            // index, seeds) runs when the build finishes, and
            // the screenshot runner pumps Spawning until then.
            // The shared chain's first system resets the door
            // state, so the screenshot app needs DoorState too —
            // the old screenshot Startup chain never included
            // reset_door_state, so the resource was missing here
            // until the chunked build unified the two paths
            // (exit 101, "Resource does not exist", 2026-11-15).
            app.init_resource::<DoorState>();
            app.add_systems(Startup, begin_screenshot_spawning);
            // Save system: a `--load-test` load runs through
            // the same worker-thread + Loading state as a door
            // transition, so the screenshot path needs poll_load
            // too (it loads its first scene synchronously and
            // never registered it before).
            app.add_systems(Update, poll_load.run_if(in_state(AppState::Loading)));
            add_scene_state_systems(&mut app);
        }
        None => {
            // M8.1: loading screen — the ESM parse runs on a
            // worker thread while the window shows "Loading..."
            // instead of a black screen for ~10 s.
            let (tx, rx) = std::sync::mpsc::channel();
            let esm = args.esm.clone();
            let data_dir = args.data.clone();
            let target = args.target.clone();
            std::thread::spawn(move || {
                let result = load_first(&esm, &data_dir, &target);
                let _ = tx.send(result);
            });
            startup_log::log("main app: parse worker thread spawned");
            app = build_windowed_app(app, bevy::DefaultPlugins, rx);
            println!(
                "controls: WASD move, mouse look, Space jump, Shift sprint, E use/talk, B bag, Esc pause/close/release mouse"
            );
        }
    }
    // M8.1: immersive fullscreen on Android — hide the status and
    // navigation bars before the first frame.
    #[cfg(target_os = "android")]
    android::apply_immersive_mode();
    startup_log::log_rss("main app: app.run() starting");
    app.run();
    startup_log::log("main app: app.run() returned (clean exit)");
    Ok(())
}

/// Resources every main-app path needs (windowed and screenshot):
/// the scene summary the HUD reads, bright interior fill light,
/// and a dark clear color (empty space reads as unlit interior
/// darkness rather than sky).
fn insert_shared_resources(app: &mut App) {
    app.init_resource::<SceneSummary>()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            // Bright interior fill (physical units, cd/m^2; Bevy's
            // default is 80). The inn has few real lights, and a
            // placeholder render must read clearly — the point
            // lights add warmth on top.
            brightness: 400.0,
            ..Default::default()
        })
        // Dark slate clear color. (During debugging this was
        // bright blue — the diagnostic that proved the camera was
        // not rendering to the target at all.)
        .insert_resource(bevy::camera::ClearColor(Color::srgb(0.08, 0.09, 0.12)));
}

/// Build the windowed main app: loading screen first, the inn
/// once the worker thread delivers the data. This is the ONLY
/// place the windowed app is constructed — `run()` and the
/// regression test both go through here, so the construction
/// order is pinned in exactly one place.
///
/// `plugins` is the base plugin set: `run()` passes
/// `bevy::DefaultPlugins`. It is a parameter (rather than
/// hardcoded) because a test cannot add `DefaultPlugins`:
/// `WinitPlugin` creates its event loop eagerly in `build` and
/// winit refuses to do that off the main thread, which is where
/// test threads run; `RenderPlugin` requests a wgpu adapter
/// during the first update (no GPU on the build machine); and
/// `CorePipelinePlugin` loads a `Shader` asset at build time,
/// which needs `RenderPlugin`. The test passes a headless set
/// (`MinimalPlugins` + `StatesPlugin` + `InputPlugin` +
/// `AssetPlugin`, with the Mesh / StandardMaterial / Image
/// storages registered) instead; the ORDER of the lines below —
/// plugins before `init_state` — is what the test pins, and it
/// is identical either way.
///
/// ORDER IS LOAD-BEARING (v0.2.3): `init_state` panics unless a
/// plugin has already registered the `StateTransition` schedule,
/// and that schedule comes from `StatesPlugin`, inside
/// `DefaultPlugins` — so the plugin set MUST be added first.
/// v0.2.0–v0.2.2 called `init_state` before `add_plugins` and
/// died during app construction, before `app.run()`, on the
/// phone (the startup log caught it red-handed: "The
/// `StateTransition` schedule is missing"). Desktop never saw
/// it: every desktop run used `--screenshot`, which takes a
/// different branch that never calls `init_state` — the windowed
/// branch was never executed on a machine with no display.
fn build_windowed_app(
    mut app: App,
    plugins: impl PluginGroup,
    rx: std::sync::mpsc::Receiver<Result<LoadResult, String>>,
) -> App {
    app.insert_resource(LoadReceiver(std::sync::Mutex::new(rx)))
        // Plugins FIRST: registers the StateTransition schedule
        // that init_state needs (see the doc comment).
        .add_plugins(plugins)
        .init_state::<AppState>()
        .init_resource::<DoorState>()
        .init_resource::<doors::DoorMemory>()
        .add_plugins(player::PlayerPlugin)
        // M17: inventory resources, doll target,
        // toggle/refresh systems, and the pickupable/container
        // indexes. The catalog builds on scene entry (below).
        // M18: dialogue resources and open/close systems; the
        // index builds on scene entry (below). UI milestone 1:
        // pause resources and open/close systems; the quest
        // index builds on scene entry (below). UI milestone 2:
        // character-menu resources and open/close systems; the
        // magic/skill/map indexes build on scene entry (below).
        .add_plugins(inventory::InventoryPlugin)
        .add_plugins(interact::InteractPlugin)
        .add_plugins(dialogue::DialoguePlugin)
        .add_plugins(pause::PausePlugin)
        .add_plugins(character::CharacterPlugin)
        .add_plugins(save::SavePlugin)
        .add_plugins(combat::CombatPlugin)
        .add_plugins(archery::ArcheryPlugin)
        // M4: the real Skyrim HUD (Ruffle). Gated at runtime by
        // the `ruffle_hud` setting (default on); the Bevy HUD
        // stays as the fallback when it is off or fails to build.
        .add_plugins(ruffle::RuffleHudPlugin);
    startup_log::log("main app: DefaultPlugins + PlayerPlugin added");
    // M8: touch controls (Android only) — Dylan's own button
    // art, virtual joystick + drag-to-look. Desktop is
    // unaffected: the plugin is never added there.
    #[cfg(target_os = "android")]
    app.add_plugins(touch::TouchControlsPlugin);
    // M8.1: re-hide the system bars whenever the window
    // regains focus (Android only).
    #[cfg(target_os = "android")]
    app.add_systems(Update, android::reapply_immersive_on_focus);
    app.add_systems(Startup, spawn_loading_screen)
        .add_systems(Update, poll_load.run_if(in_state(AppState::Loading)))
        // v0.2.2: first-frame marker — logs on ticks 1 and 2.
        .add_systems(Update, log_frame_tick_system)
        // M9: walking through a door loads the scene on the other
        // side. Runs after movement so it sees this frame's
        // position, InGame only (never during a load). M15: USE
        // toggles the nearest door, the swing animates toward its
        // target, and only then does the transition check the
        // crossing — chained in that order. M17: USE also picks
        // up loose items and loots containers — after the door
        // system, before the swing, and each skips the press when
        // a higher-priority target is in range. M18: USE on an
        // NPC opens dialogue — between doors and pickup (door >
        // dialogue > pickup > container), so one press does one
        // thing.
        .add_systems(
            Update,
            (
                doors::door_use,
                dialogue::dialogue_use,
                interact::pickup_use,
                interact::container_use,
                doors::door_swing,
                door_transition,
            )
                .chain()
                .run_if(in_state(AppState::InGame))
                .after(player::keyboard_move),
        );
    // The scene state machine (Loading -> Spawning -> InGame)
    // is shared with the screenshot path — see
    // `add_scene_state_systems`.
    add_scene_state_systems(&mut app);
    startup_log::log("main app: systems added (loading screen, poll, frame tick)");
    app
}

/// v0.2.2: first-frame marker system — logs on Update ticks 1 and
/// 2 (tick 2 proves the first frame rendered). Added to the main
/// app and the message app.
pub(crate) fn log_frame_tick_system() {
    startup_log::log_frame_tick();
}

/// First load: parse the ESM, index the data folder, load the
/// target scene, compute bounds. Used synchronously by the
/// screenshot path and on a worker thread by the windowed path
/// (M8.1 loading screen). Prints progress as it goes (the phone's
/// logcat shows it too). The parsed plugin and the provider come
/// back with the scene — both are kept for door transitions (M9).
fn load_first(esm: &Path, data: &Path, target: &SceneTarget) -> Result<LoadResult, String> {
    // v0.2.2: runs on the worker thread — every stage logs, and a
    // failure returns an error (shown on the loading screen),
    // never a panic (which would only disconnect the channel).
    println!("loading {} ...", esm.display());
    startup_log::log_rss(&format!("load: ESM parse start ({})", esm.display()));
    let plugin = Arc::new(dovah_esm::Plugin::open(esm).map_err(|e| {
        startup_log::log(&format!("load: ESM parse FAILED: {e}"));
        format!("failed to parse {}: {e}", esm.display())
    })?);
    let records = plugin
        .header_info()
        .map(|h| h.record_count)
        .unwrap_or_default();
    startup_log::log_rss(&format!("load: ESM parse done ({records} records)"));
    let mut provider = AssetProvider::new(data);
    startup_log::log(&format!(
        "load: BSA discovery — {} archive(s), {} files indexed: {}",
        provider.archive_names.len(),
        provider.indexed_files(),
        provider.archive_names.join(", ")
    ));
    print_provider(data, &provider);
    // M18: the localized strings come from Patch.bsa, through
    // the same provider — no separate Strings folder on the
    // phone. A missing file degrades to an empty set (names
    // fall back to prettified EDIDs), never a failed load.
    startup_log::log("load: strings start");
    let strings = dialogue::GameStrings::load(&mut provider);
    startup_log::log("load: strings done");
    // An exterior first load (desktop `--exterior`) starts where
    // the player would step out of the inn: the inn door's XTEL
    // arrival point.
    let spawn = match target {
        SceneTarget::Exterior { .. } => cell::find_xtel_spawn(&plugin, "RiverwoodSleepingGiantInn"),
        SceneTarget::Interior { .. } => None,
    };
    let data = load_scene(&plugin, target, spawn)?;
    Ok(LoadResult::First {
        plugin,
        provider,
        strings,
        data,
    })
}

/// Load one scene from an already-parsed plugin: the cell or
/// exterior block, plus its bounds. Used by [`load_first`] and by
/// door transitions (M9), which reuse the parsed plugin instead
/// of re-parsing the ESM.
fn load_scene(
    plugin: &dovah_esm::Plugin,
    target: &SceneTarget,
    spawn: Option<SpawnPoint>,
) -> Result<LoadedData, String> {
    let cell_data = match target {
        SceneTarget::Interior { edid, .. } => {
            cell::load_cell(plugin, edid, spawn).ok_or_else(|| {
                startup_log::log(&format!("load: no CELL with EDID '{edid}'"));
                format!("no CELL with EDID '{edid}'")
            })?
        }
        SceneTarget::Exterior { grid } => {
            cell::load_exterior(plugin, *grid, spawn).ok_or_else(|| {
                startup_log::log(&format!(
                    "load: no exterior CELL at grid ({}, {})",
                    grid.0, grid.1
                ));
                format!("no exterior CELL at grid ({}, {})", grid.0, grid.1)
            })?
        }
    };
    println!(
        "cell {} (form 0x{:08x}, interior={}): {} objects to draw, {} terrain cells",
        cell_data.edid,
        cell_data.form_id,
        cell_data.interior,
        cell_data.objects.len(),
        cell_data.lands.len()
    );
    if !cell_data.lands.is_empty() {
        let mut min_h = f32::INFINITY;
        let mut max_h = f32::NEG_INFINITY;
        for land in &cell_data.lands {
            let (lo, hi) = land.height_range();
            min_h = min_h.min(lo);
            max_h = max_h.max(hi);
        }
        println!(
            "terrain heights: {:.1}..{:.1} Skyrim units ({:.1}..{:.1} m)",
            min_h,
            max_h,
            min_h / coords::SKYRIM_UNITS_PER_METER,
            max_h / coords::SKYRIM_UNITS_PER_METER
        );
    }
    startup_log::log(&format!(
        "load: cell {} (form 0x{:08x}, interior={}), {} objects, {} terrain cells, {} doors",
        cell_data.edid,
        cell_data.form_id,
        cell_data.interior,
        cell_data.objects.len(),
        cell_data.lands.len(),
        cell_data.doors.len()
    ));
    let bounds = match target {
        SceneTarget::Interior { .. } => RoomBounds::from_cell(&cell_data),
        SceneTarget::Exterior { grid } => {
            // Floor level outdoors: the terrain height at the
            // block's centre (the median object height is
            // meaningless across hills).
            let floor_y = cell_data
                .lands
                .iter()
                .find(|l| l.grid == *grid)
                .and_then(|l| l.sample_height(2048.0, 2048.0))
                .map(|h| h / coords::SKYRIM_UNITS_PER_METER)
                .unwrap_or(0.0);
            RoomBounds::from_exterior(*grid, floor_y)
        }
    };
    println!(
        "room bounds: min {:?} max {:?} floor_y {:.2} m",
        bounds.min, bounds.max, bounds.floor_y
    );
    startup_log::log_rss("load: data ready, sending to main thread");
    Ok(LoadedData { cell_data, bounds })
}

fn print_provider(data: &Path, provider: &AssetProvider) {
    println!(
        "data folder {}: {} archive(s), {} files indexed",
        data.display(),
        provider.archive_names.len(),
        provider.indexed_files()
    );
    for name in &provider.archive_names {
        println!("  archive: {name}");
    }
}

/// The loading screen: dark full-screen node with a title and a
/// subtext. Spawns in Startup for the windowed path; the touch UI
/// (Android) spawns first, so this renders above it, and is
/// despawned when the data arrives. Door transitions (M9) respawn
/// it with the destination's name via [`spawn_loading_screen_ui`].
fn spawn_loading_screen(mut commands: Commands, assets: Res<AssetServer>) {
    // v0.2.2: proves the Startup schedule ran (the v0.2.0/v0.2.1
    // crash happened before the first frame — if this line is
    // missing, the app died during app construction or Startup).
    startup_log::log("loading screen spawned (Startup ran)");
    spawn_loading_screen_ui(
        &mut commands,
        &assets,
        "Loading the Sleeping Giant Inn...",
        "Parsing Skyrim.esm (238 MB) - this takes a few seconds the first time.",
        true,
    );
}

/// M15 show build: the loading screen carries Dylan's picked
/// TITLED DovahEngine logo (dragon sigil + "DOVAHENGINE" text,
/// `assets/ui/dovahengine-logo.png` — the same art as the APK
/// icon) above the title. The sigil-only version is NOT the pick
/// — do not swap it in.
fn spawn_loading_screen_ui(
    commands: &mut Commands,
    assets: &AssetServer,
    title: &str,
    subtext: &str,
    with_camera: bool,
) {
    // v0.4.4 phone frames: the door transition froze on the inn
    // view — the loading screen never presented — because the
    // game camera is despawned on transition and NOTHING renders
    // UI without a camera. Same hole at startup (the grey
    // screen): no camera exists until the first InGame camera
    // spawns. A 2D camera with the LoadingScreen marker (so
    // poll_load despawns it with the screen) closes it. The
    // screenshot path passes false: it already has a camera.
    if with_camera {
        commands.spawn((
            bevy::camera::Camera2d,
            bevy::ui::IsDefaultUiCamera,
            LoadingScreen,
        ));
    }
    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Vh(2.0),
                ..Default::default()
            },
            BackgroundColor(Color::srgb(0.05, 0.05, 0.07)),
            LoadingScreen,
        ))
        .with_children(|screen| {
            // The logo is square (1254x1254); 36vh keeps it
            // prominent on a phone portrait screen without
            // crowding the title below it.
            screen.spawn((
                ImageNode::new(assets.load("ui/dovahengine-logo.png")),
                Node {
                    width: Val::Vh(36.0),
                    height: Val::Vh(36.0),
                    ..Default::default()
                },
            ));
            screen.spawn((
                Text::new(title.to_string()),
                TextFont {
                    font_size: FontSize::Px(42.0),
                    ..Default::default()
                },
                TextColor(Color::WHITE),
                LoadingTitle,
            ));
            screen.spawn((
                Text::new(subtext.to_string()),
                TextFont {
                    font_size: FontSize::Px(22.0),
                    ..Default::default()
                },
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.7)),
                LoadingText,
            ));
            // The chunked build's progress line (empty during
            // the ESM parse — `update_loading_progress` fills
            // it once Spawning starts: "Spawning objects
            // 123/437").
            screen.spawn((
                Text::new(String::new()),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..Default::default()
                },
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.6)),
                LoadingProgress,
            ));
        });
}

/// Marker resource (M15 show-build proof hook): present only
/// when `--screenshot-loading` was passed.
#[derive(Resource)]
struct ScreenshotLoading;

/// M15 show-build proof hook: draws the loading screen (with
/// the DovahEngine logo) into a screenshot. The screenshot path
/// has no Loading state, so nothing despawns it — the capture
/// shows exactly what the player sees on startup, logo and all.
/// No-ops unless the `ScreenshotLoading` marker resource is
/// present.
fn spawn_screenshot_loading_screen(
    mut commands: Commands,
    assets: Res<AssetServer>,
    flag: Option<Res<ScreenshotLoading>>,
) {
    if flag.is_none() {
        return;
    }
    spawn_loading_screen_ui(
        &mut commands,
        &assets,
        "Loading the Sleeping Giant Inn...",
        "Parsing Skyrim.esm (238 MB) - this takes a few seconds the first time.",
        false,
    );
}

/// Poll the worker thread's channel (Loading state only). On
/// success the loaded data becomes resources and the state flips
/// to Spawning — the loading screen STAYS ALIVE through the
/// chunked build (with progress) and comes down on InGame entry
/// (`despawn_loading_screen`). The old code despawned it here
/// and entered InGame straight into a 22–32 s synchronous scene
/// build — the ANR. On failure the screen shows the error
/// instead of a black window.
fn poll_load(
    mut commands: Commands,
    receiver: Option<ResMut<LoadReceiver>>,
    mut next_state: ResMut<NextState<AppState>>,
    // v0.2.3: the two `&mut Text` queries need `Without` filters
    // to be provably disjoint — Bevy cannot tell from `With`
    // alone that no entity carries both markers, and panics at
    // system init with B0001 (a third latent crash in the
    // windowed path, found by the v0.2.3 regression test — the
    // loading screen spawns the title and the subtext as
    // separate entities, so the filters match reality).
    mut title: Query<&mut Text, (With<LoadingTitle>, Without<LoadingText>)>,
    mut subtext: Query<&mut Text, (With<LoadingText>, Without<LoadingTitle>)>,
) {
    let Some(receiver) = receiver else {
        return;
    };
    match receiver.try_recv() {
        Ok(Ok(LoadResult::First {
            plugin,
            provider,
            strings,
            data,
        })) => {
            startup_log::log_rss("load: first data received, entering Spawning");
            commands.insert_resource(data.cell_data);
            commands.insert_resource(provider);
            commands.insert_resource(strings);
            commands.insert_resource(data.bounds);
            commands.insert_resource(SharedPlugin(plugin));
            commands.remove_resource::<LoadReceiver>();
            next_state.set(AppState::Spawning);
        }
        Ok(Ok(LoadResult::Scene { data })) => {
            startup_log::log_rss("load: transition data received, entering Spawning");
            commands.insert_resource(data.cell_data);
            commands.insert_resource(data.bounds);
            commands.remove_resource::<LoadReceiver>();
            next_state.set(AppState::Spawning);
        }
        Ok(Err(e)) => {
            startup_log::log(&format!("load: worker reported error: {e}"));
            show_load_error(&mut title, &mut subtext, &e);
            commands.remove_resource::<LoadReceiver>();
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            // The worker died without sending (a panic). Say so.
            startup_log::log("load: worker thread died without sending (panic in load?)");
            show_load_error(
                &mut title,
                &mut subtext,
                "The loading thread stopped unexpectedly.",
            );
            commands.remove_resource::<LoadReceiver>();
        }
    }
}

/// Despawn the loading screen (UI root + its camera) when the
/// scene build finishes and the app enters InGame. The screen
/// stays alive through Loading AND Spawning — the chunked build
/// shows progress on it — and comes down here, first in the
/// OnEnter(InGame) chain, so the first InGame frame is the
/// scene, not the screen.
fn despawn_loading_screen(
    mut commands: Commands,
    loading_root: Query<Entity, With<LoadingScreen>>,
) {
    for entity in &loading_root {
        commands.entity(entity).despawn();
    }
}

/// Update the loading screen's progress line from the chunked
/// build (Spawning only): "Spawning objects 123/437". No-ops
/// when the progress or the text is absent (the screenshot path
/// has no loading screen) — a UI nicety is never worth a panic.
fn update_loading_progress(
    progress: Option<Res<scene::SpawnProgress>>,
    mut text: Query<&mut Text, With<LoadingProgress>>,
) {
    let Some(progress) = progress else {
        return;
    };
    if let Ok(mut text) = text.single_mut() {
        text.0 = progress.progress_text();
    }
}

/// The scene state machine, shared by the windowed path and the
/// screenshot path (one implementation — the screenshot runner
/// pumps Spawning until the build is done, then captures).
/// Loading -> Spawning (chunked build, loading screen alive
/// with progress) -> InGame (camera + catalog + indexes).
/// `--drop-test` seed: item base form IDs to give and then drop
/// on the first InGame frame (drop-diagnosis proof hook).
/// Applied by `apply_drop_test_seed` in the OnEnter(InGame)
/// chain, after the inventory seed.
#[derive(Resource)]
struct DropTestSeed(Vec<u32>);

/// `--drop-test` proof hook: gives each listed item to the
/// inventory, then drops it through the REAL `drop_item` path —
/// the dropped meshes land exactly where the phone's drops land
/// (feet + forward*0.8), so the screenshot shows what the
/// player sees. No-op without the seed resource (absent on the
/// phone — Android has no CLI flags).
#[allow(clippy::too_many_arguments)]
fn apply_drop_test_seed(
    mut commands: Commands,
    seed: Option<Res<DropTestSeed>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    assets: Option<ResMut<AssetProvider>>,
    catalog: Res<inventory::ItemCatalog>,
    mut inventory: ResMut<inventory::Inventory>,
    mut pickupables: ResMut<interact::Pickupables>,
    mut dropped_ids: ResMut<inventory::DroppedRefIds>,
    mut dirty: ResMut<inventory::DollDirty>,
    player: Query<&Transform, With<Player>>,
) {
    let Some(seed) = seed else {
        return;
    };
    let Some(mut assets) = assets else {
        return;
    };
    let Ok(player_transform) = player.single() else {
        return;
    };
    let player_transform = *player_transform;
    for base in &seed.0 {
        inventory.add(*base, 1);
        let selected = inventory::SelectedItem(Some(*base));
        inventory::drop_item(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            &mut assets,
            &catalog,
            &mut inventory,
            &selected,
            &mut pickupables,
            &mut dropped_ids,
            &mut dirty,
            &player_transform,
        );
    }
}

fn add_scene_state_systems(app: &mut App) {
    app.add_systems(OnEnter(AppState::Spawning), scene::begin_spawning)
        .add_systems(
            Update,
            (scene::spawn_chunk, update_loading_progress)
                .chain()
                .run_if(in_state(AppState::Spawning)),
        )
        // M17: the catalog builds once per app run from the
        // parsed plugin (re-entry is a no-op — the starting
        // outfit is seeded once), the interactables re-index
        // every scene load, and the `--give` seed applies on the
        // first entry (desktop proof hook; absent on the phone).
        // M18: the dialogue index builds once (topics,
        // greetings, names), the talkable NPCs re-index every
        // scene load, and the `--dialogue` seed opens Delphine's
        // dialogue on the first entry (desktop proof hook).
        // UI milestone 1: the quest index builds once (the
        // journal's quest list), and the `--pause` seed opens
        // the pause screen on the first entry (desktop proof
        // hook). UI milestone 2: the magic/skill indexes build
        // once (the character menu's data), and the
        // `--character-menu` seed opens the character menu on
        // the first entry (desktop proof hook). The MAP index
        // does NOT build here: the one-shot bake took ~7 s on
        // the main thread (an ANR) — it bakes in the Spawning
        // MapBake phase instead, a budget slice at a time, so
        // the map is always built by the time InGame starts.
        // The loading
        // screen comes down first — the build is done, the
        // first InGame frame is the scene.
        // The `--screenshot-loading` hook re-spawns it last,
        // for the logo proof shot (no-op without the marker).
        .add_systems(
            OnEnter(AppState::InGame),
            (
                reset_door_state,
                despawn_loading_screen,
                scene::spawn_camera,
                inventory::build_catalog,
                interact::build_interactables,
                // Save system: re-apply the world changes to the
                // freshly built indexes (taken items stay taken
                // across loads and door transitions).
                save::apply_world_changes,
                dialogue::build_dialogue_index,
                dialogue::build_talkable_npcs,
                combat::build_combatants,
                combat::apply_dead_npcs,
                pause::build_quest_index,
                character::build_magic_index,
                character::build_skill_index,
                inventory::apply_inventory_seed,
                apply_drop_test_seed,
                dialogue::apply_dialogue_seed,
                combat::apply_melee_test_seed,
                // The chain is 21 systems — past Bevy's
                // 20-element tuple limit — so the tail nests
                // in a sub-tuple (Bevy flattens it; the order
                // is unchanged).
                (
                    archery::apply_archery_test_seed,
                    pause::apply_pause_seed,
                    character::apply_character_seed,
                    spawn_screenshot_loading_screen,
                ),
            )
                .chain(),
        );
}

/// Startup system for the screenshot path: the data is already
/// loaded synchronously (no worker thread, no loading screen),
/// so the first update starts the chunked build directly.
fn begin_screenshot_spawning(mut next_state: ResMut<NextState<AppState>>) {
    next_state.set(AppState::Spawning);
}

/// Door-trigger state for transitions (M9).
#[derive(Resource, Default)]
struct DoorState {
    /// Door ref form IDs the player is currently inside the
    /// trigger radius of.
    inside: HashSet<u32>,
    /// Frames left in the post-arrival grace period. The player
    /// spawns inside the door trigger on arrival (the XTEL spawn
    /// sits ~1 m from the door ref), so the first frames only
    /// update the inside-set — no transition fires until the
    /// grace expires, by which time the player is recorded as
    /// inside and must walk out and back in to trigger.
    grace_frames: u32,
    /// The player's feet position last frame (M15): plane
    /// crossings are computed from prev -> now. `None` until the
    /// first frame after a transition — `reset_door_state` clears
    /// it, so no stale crossing can fire on arrival.
    prev_feet: Option<bevy::math::Vec3>,
}

/// Horizontal trigger radius around a door ref, in metres. The
/// inn door's XTEL spawn is ~1.1 m from the door ref outside and
/// ~0.9 m inside — both inside this radius on arrival, which is
/// why the grace period + edge detection matter.
const DOOR_TRIGGER_RADIUS_M: f32 = 1.2;
/// Vertical tolerance for a door trigger (the player's feet vs
/// the door ref's height), in metres.
const DOOR_TRIGGER_HEIGHT_M: f32 = 2.0;
/// Post-arrival grace frames (30 frames at 60 fps = 0.5 s).
const DOOR_GRACE_FRAMES: u32 = 30;

/// Reset the door state when a scene is entered (OnEnter InGame,
/// before the scene spawns).
fn reset_door_state(mut state: ResMut<DoorState>) {
    state.inside.clear();
    state.grace_frames = DOOR_GRACE_FRAMES;
    state.prev_feet = None;
}

/// Everything a door transition despawns: the whole spawned
/// scene plus the player (and its camera, parented to it).
type DespawnOnTransition<'w, 's> = Query<'w, 's, Entity, Or<(With<SceneEntity>, With<Player>)>>;

/// Pure inside-test for a door trigger (M9): the player's feet
/// are within the trigger radius horizontally and the trigger
/// height vertically. Split out of `door_transition` so the
/// geometry is unit-testable without a Bevy app. Both positions
/// are in Bevy metres; the feet position carries the player's
/// feet height, not the eye height.
fn inside_door_trigger(trigger: bevy::math::Vec3, feet: bevy::math::Vec3) -> bool {
    let dx = feet.x - trigger.x;
    let dz = feet.z - trigger.z;
    let horizontal = (dx * dx + dz * dz).sqrt();
    let vertical = (feet.y - trigger.y).abs();
    horizontal < DOOR_TRIGGER_RADIUS_M && vertical < DOOR_TRIGGER_HEIGHT_M
}

/// Walk through a door, load the scene on the other side (M9).
/// Swinging doors (M15) fire on CROSSING the doorway plane while
/// the door is open enough — never through a closed door, and
/// never the moment USE is pressed (the player must walk through;
/// a proximity edge + open-gate would teleport them while they
/// still stood inside the 1.2 m radius). Doors with no swinging
/// panel keep the M9 proximity edge: a transition fires only when
/// the player ENTERS the trigger radius (was outside last frame,
/// inside this frame), never while standing inside it — so
/// arriving through a door never bounces the player straight
/// back. On firing: the scene entities and the player/camera
/// despawn, a worker thread loads the destination from the shared
/// parsed plugin, the loading screen comes back, and the state
/// flips to Loading. `poll_load` takes it from there.
#[allow(clippy::too_many_arguments)]
fn door_transition(
    mut commands: Commands,
    cell: Option<Res<cell::CellData>>,
    plugin: Option<Res<SharedPlugin>>,
    mut door_state: ResMut<DoorState>,
    doors: Option<Res<doors::Doors>>,
    player: Query<&Transform, With<Player>>,
    scene_entities: DespawnOnTransition<'_, '_>,
    mut next_state: ResMut<NextState<AppState>>,
    assets: Res<AssetServer>,
    save_dir: Option<Res<save::SaveDir>>,
    capture: save::SaveCaptureParams<'_, '_>,
) {
    let Some(cell) = cell else {
        return;
    };
    if cell.doors.is_empty() {
        return;
    }
    let Ok(player_transform) = player.single() else {
        return;
    };
    let feet_y = player_transform.translation.y - player::EYE_HEIGHT;
    let feet = bevy::math::Vec3::new(
        player_transform.translation.x,
        feet_y,
        player_transform.translation.z,
    );
    // Tick the grace period down; while it runs, update the
    // inside-set but fire nothing.
    let firing = door_state.grace_frames == 0;
    if door_state.grace_frames > 0 {
        door_state.grace_frames -= 1;
    }
    let prev_feet = door_state.prev_feet;
    let mut fired: Option<(SceneTarget, SpawnPoint)> = None;
    for door in &cell.doors {
        match doors.as_deref().and_then(|d| d.get(door.ref_form_id)) {
            // M15 swinging door: cross the plane while open.
            // The decision itself is the pure, unit-tested
            // `swinging_door_fires` (doors.rs) — closed doors
            // never fire, open doors fire only on a crossing.
            Some(runtime) => {
                if firing && doors::swinging_door_fires(runtime, prev_feet, feet) {
                    fired = Some((door.target.clone(), door.spawn));
                    break;
                }
            }
            // No swinging panel: the M9 proximity edge.
            None => {
                let trigger = coords::to_bevy_position(door.trigger_pos);
                let inside = inside_door_trigger(trigger, feet);
                let was_inside = door_state.inside.contains(&door.ref_form_id);
                if inside {
                    door_state.inside.insert(door.ref_form_id);
                } else {
                    door_state.inside.remove(&door.ref_form_id);
                }
                if firing && inside && !was_inside {
                    fired = Some((door.target.clone(), door.spawn));
                    break;
                }
            }
        }
    }
    door_state.prev_feet = Some(feet);
    let Some((target, spawn)) = fired else {
        return;
    };
    let Some(plugin) = plugin else {
        return;
    };
    startup_log::log(&format!(
        "door transition: loading {}",
        target.display_name()
    ));
    // Save system: Skyrim autosaves on door transition. The
    // capture is the pre-transition state (at the doorway) —
    // safe to load back, because the 30-frame grace period +
    // the proximity edge / swinging-door crossing rule mean
    // loading it never fires the transition again at once.
    if let Some(dir) = &save_dir {
        save::write_autosave_now(&dir.0, &capture);
    }
    // Despawn the old scene and the player/camera; OnEnter(InGame)
    // spawns them fresh for the new scene.
    for entity in &scene_entities {
        commands.entity(entity).despawn();
    }
    // Load the destination on a worker thread from the shared
    // parsed plugin (no ESM re-parse, no BSA re-index).
    let title = format!("Loading {}...", target.display_name());
    let (tx, rx) = std::sync::mpsc::channel();
    let shared = plugin.0.clone();
    std::thread::spawn(move || {
        let result =
            load_scene(&shared, &target, Some(spawn)).map(|data| LoadResult::Scene { data });
        let _ = tx.send(result);
    });
    commands.insert_resource(LoadReceiver(std::sync::Mutex::new(rx)));
    spawn_loading_screen_ui(
        &mut commands,
        &assets,
        &title,
        "Walking through the door...",
        true,
    );
    next_state.set(AppState::Loading);
}

fn show_load_error(
    title: &mut Query<&mut Text, (With<LoadingTitle>, Without<LoadingText>)>,
    subtext: &mut Query<&mut Text, (With<LoadingText>, Without<LoadingTitle>)>,
    message: &str,
) {
    if let Ok(mut text) = title.single_mut() {
        text.0 = "Couldn't load the area".to_string();
    }
    if let Ok(mut text) = subtext.single_mut() {
        text.0 = message.to_string();
    }
}

/// Entry point. On Android `#[bevy_main]` generates `android_main`,
/// which sets `bevy::android::ANDROID_APP` (winit and the asset
/// reader need it) and then calls this.
///
/// Returns `()` rather than `ExitCode`: the generated
/// `android_main` discards the return value, which trips
/// `unused_must_use` on Android. Failures exit the process
/// directly, so the desktop binary still gets a nonzero status.
#[bevy::prelude::bevy_main]
pub fn main() {
    // v0.2.2: the panic hook and the startup log come FIRST —
    // before any Bevy/Android init — so a crash during startup is
    // recorded, not guessed at. The log lives in Downloads (or the
    // app's files folder when permission is denied) as
    // `dovahengine-startup.log`; Dylan sends that file to Jarvis.
    startup_log::install_panic_hook();
    let log_path = startup_log::init();
    startup_log::log_rss(&format!(
        "main entry (dovah-app v{}), log at {}",
        env!("CARGO_PKG_VERSION"),
        log_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<no writable log dir>".to_string())
    ));
    #[cfg(target_os = "android")]
    startup_log::log(&format!(
        "ANDROID_APP present: {}",
        bevy::android::ANDROID_APP.get().is_some()
    ));
    if let Err(e) = run() {
        startup_log::log(&format!("run() failed: {e}"));
        eprintln!("dovah-app: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_receiver_survives_a_poisoned_lock() {
        let (_tx, rx) = std::sync::mpsc::channel::<Result<LoadResult, String>>();
        let mutex = std::sync::Mutex::new(rx);
        // Poison the lock: a thread locks it and panics while
        // holding it. The panic is caught by `join`, not the test.
        let _ = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _guard = mutex.lock().unwrap();
                    panic!("poison the load-channel lock");
                })
                .join()
        });
        assert!(mutex.is_poisoned());
        let receiver = LoadReceiver(mutex);
        // Must not panic: the channel is empty, the sender alive.
        assert!(matches!(
            receiver.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
    }

    /// v0.2.3 regression test: the windowed app must be built
    /// through [`build_windowed_app`] with the plugin set added
    /// BEFORE `init_state`. v0.2.0–v0.2.2 had the order reversed
    /// and panicked during app construction on the phone ("The
    /// `StateTransition` schedule is missing") — before
    /// `app.run()`, before the first frame. Desktop never caught
    /// it: every desktop run used `--screenshot`, which takes a
    /// branch that never calls `init_state`. This test builds the
    /// app exactly the way `run()` does (shared resources +
    /// `build_windowed_app`) and runs two updates in the Loading
    /// state — the reversed order panics here at build time, with
    /// no display or GPU needed. The plugin set is headless
    /// (`MinimalPlugins` + `StatesPlugin` + `InputPlugin`):
    /// `DefaultPlugins` cannot run in a test — `WinitPlugin`
    /// creates its event loop eagerly and refuses off the main
    /// thread, `RenderPlugin` requests a wgpu adapter (no GPU on
    /// this VM), and `CorePipelinePlugin` loads a `Shader` asset
    /// at build time. What the test pins is the ORDER of the
    /// lines in `build_windowed_app` — plugins before
    /// `init_state` — which is identical either way, plus the two
    /// updates running in the Loading state. The RoomBounds fix
    /// (v0.2.2) is what lets the two updates run:
    /// `keyboard_move` tolerates the missing bounds until the
    /// data arrives.
    #[test]
    fn windowed_app_builds_and_runs_two_updates() {
        let (_tx, rx) = std::sync::mpsc::channel::<Result<LoadResult, String>>();
        let mut app = App::new();
        insert_shared_resources(&mut app);
        // M17: the inventory plugin needs the asset storages
        // (Assets<Image> for the doll target, Assets<Mesh> /
        // Assets<StandardMaterial> for the doll) — the real app
        // gets them from DefaultPlugins. MinimalPlugins
        // has no AssetPlugin, so the headless set adds it and
        // registers the three storages directly (all headless:
        // no GPU, no window).
        let plugins = bevy::MinimalPlugins
            .build()
            .add(bevy::state::app::StatesPlugin)
            .add(bevy::input::InputPlugin)
            .add(bevy::asset::AssetPlugin::default());
        let mut app = build_windowed_app(app, plugins, rx);
        {
            use bevy::asset::AssetApp;
            app.init_asset::<bevy::mesh::Mesh>()
                .init_asset::<bevy::pbr::StandardMaterial>()
                .init_asset::<bevy::image::Image>();
        }
        // Loading state, no data delivered: two updates must not
        // panic (construction order + RoomBounds tolerance).
        app.update();
        app.update();
        assert_eq!(
            *app.world()
                .resource::<bevy::prelude::State<AppState>>()
                .get(),
            AppState::Loading
        );
        // The loading screen carries its own camera (v0.4.4: the
        // door freeze showed the inn, not the loading screen —
        // the game camera is despawned on transition and UI
        // renders through nothing without a camera).
        let mut camera_query = app.world_mut().query::<(
            Entity,
            &bevy::camera::Camera2d,
            &bevy::ui::IsDefaultUiCamera,
        )>();
        let cameras: Vec<_> = camera_query.iter(app.world()).collect();
        assert_eq!(cameras.len(), 1, "exactly one loading camera");
        assert!(
            app.world().get::<LoadingScreen>(cameras[0].0).is_some(),
            "the loading camera carries the LoadingScreen marker so poll_load despawns it"
        );
    }

    #[test]
    fn chunked_build_transitions_spawning_to_ingame_with_counts() {
        // The ANR fix, end to end on the headless app: a fake
        // cell builds chunked (one object per frame — the budget
        // is zeroed) through Loading -> Spawning -> InGame, the
        // summary counts match the fake cell, and the loading
        // screen comes down on arrival. The fake cell has 5
        // objects whose NIFs do not exist (placeholders), 1
        // model-less light, and 1 actor with no resolvable
        // models (capsule). The provider points at a nonexistent
        // dir, so every file read misses — the same path a
        // missing mesh takes.
        let (_tx, rx) = std::sync::mpsc::channel::<Result<LoadResult, String>>();
        let mut app = App::new();
        insert_shared_resources(&mut app);
        let plugins = bevy::MinimalPlugins
            .build()
            .add(bevy::state::app::StatesPlugin)
            .add(bevy::input::InputPlugin)
            .add(bevy::asset::AssetPlugin::default());
        let mut app = build_windowed_app(app, plugins, rx);
        {
            use bevy::asset::AssetApp;
            app.init_asset::<bevy::mesh::Mesh>()
                .init_asset::<bevy::pbr::StandardMaterial>()
                .init_asset::<bevy::image::Image>();
        }
        let mut objects = Vec::new();
        for i in 0..5u32 {
            objects.push(cell::PlacedObject {
                ref_form_id: 0x1000 + i,
                base_form_id: 0x2000 + i,
                base_type: "STAT".to_string(),
                base_edid: format!("FakeStat{i:02}"),
                model_path: Some("Missing\\NoSuchModel.nif".to_string()),
                position: [i as f32 * 70.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0],
                scale: 1.0,
                bounds: None,
                light: None,
                is_actor: false,
                is_creature: false,
                actor_models: Vec::new(),
                xtel: None,
            });
        }
        objects.push(cell::PlacedObject {
            ref_form_id: 0x1005,
            base_form_id: 0x2005,
            base_type: "LIGH".to_string(),
            base_edid: "FakeLight01".to_string(),
            model_path: None,
            position: [0.0, 0.0, 350.0],
            rotation: [0.0, 0.0, 0.0],
            scale: 1.0,
            bounds: None,
            light: Some(cell::LightData {
                radius: 512,
                color: [255, 200, 100],
                flags: 0x9,
                falloff: 1.0,
                fade: 1.0,
            }),
            is_actor: false,
            is_creature: false,
            actor_models: Vec::new(),
            xtel: None,
        });
        objects.push(cell::PlacedObject {
            ref_form_id: 0x1006,
            base_form_id: 0x2006,
            base_type: "NPC_".to_string(),
            base_edid: "FakeActor".to_string(),
            model_path: None,
            position: [0.0, 0.0, -350.0],
            rotation: [0.0, 0.0, 0.0],
            scale: 1.0,
            bounds: None,
            light: None,
            is_actor: true,
            is_creature: false,
            actor_models: Vec::new(),
            xtel: None,
        });
        app.insert_resource(cell::CellData {
            edid: "FakeCell".to_string(),
            form_id: 0x1234,
            interior: true,
            objects,
            lands: Vec::new(),
            waters: Vec::new(),
            spawn: None,
            doors: Vec::new(),
            skipped_markers: 0,
            skipped_disabled: 0,
            skipped_no_data: 0,
            skipped_no_base: 0,
            skipped_no_model: 0,
            skipped_other_types: 0,
            skipped_effects: 0,
        });
        app.insert_resource(scene::RoomBounds {
            min: bevy::math::Vec3::new(-10.0, -5.0, -10.0),
            max: bevy::math::Vec3::new(10.0, 10.0, 10.0),
            floor_y: 0.0,
        });
        app.insert_resource(assets::AssetProvider::new(std::path::Path::new(
            "/nonexistent/dovah-chunk-test",
        )));
        // Zero budget: exactly one object per frame, so the
        // build PROVABLY splits across frames (7 objects + a
        // dressing frame + a finish frame, plus one frame for
        // the progress resource to land).
        app.insert_resource(scene::SpawnBudget(std::time::Duration::ZERO));
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Spawning);
        let mut saw_spawning = false;
        let mut frames = 0;
        // The loading screen's progress line, sampled each
        // Spawning frame: the user-visible half of the ANR fix
        // (the screen stays alive and counts objects while the
        // build runs, instead of freezing on the last frame).
        let mut progress_texts: Vec<String> = Vec::new();
        for _ in 0..30 {
            app.update();
            frames += 1;
            let state = *app
                .world()
                .resource::<bevy::prelude::State<AppState>>()
                .get();
            if state == AppState::Spawning {
                saw_spawning = true;
                let mut text_query = app
                    .world_mut()
                    .query_filtered::<&Text, With<LoadingProgress>>();
                if let Some(text) = text_query.iter(app.world()).next() {
                    progress_texts.push(text.0.clone());
                }
            }
            if state == AppState::InGame {
                break;
            }
        }
        assert!(saw_spawning, "the build never entered Spawning");
        // The loading screen counted objects mid-build: at least
        // one sampled line is a genuine in-progress count
        // ("Spawning objects N/7", 0 < N < 7) — not empty, not
        // already Done. (Zero budget: one object per frame, so
        // the counts walk 1/7, 2/7, ... across the sampled
        // frames.)
        let mid_build = progress_texts.iter().any(|t| {
            t.strip_prefix("Spawning objects ")
                .and_then(|rest| rest.strip_suffix("/7"))
                .and_then(|n| n.parse::<usize>().ok())
                .map(|n| (1..7).contains(&n))
                .unwrap_or(false)
        });
        assert!(
            mid_build,
            "no mid-build progress line observed; sampled: {progress_texts:?}"
        );
        assert!(
            frames > 1,
            "a zero-budget build of 7 objects finished in {frames} frame(s) — the chunking did not split it"
        );
        assert_eq!(
            *app.world()
                .resource::<bevy::prelude::State<AppState>>()
                .get(),
            AppState::InGame,
            "the build did not finish within 30 frames"
        );
        let summary = app.world().resource::<scene::SceneSummary>();
        assert_eq!(summary.objects_total, 7);
        assert_eq!(summary.placeholder_objects, 5);
        assert_eq!(summary.lights, 1);
        assert_eq!(summary.actors, 1);
        assert_eq!(summary.actor_parts, 0);
        assert_eq!(summary.real_mesh_objects, 0);
        assert_eq!(summary.meshes_not_found, 5);
        // The progress record is Done at 100%.
        let progress = app.world().resource::<scene::SpawnProgress>();
        assert_eq!(progress.phase, scene::SpawnPhase::Done);
        assert_eq!(progress.progress(), 1.0);
        // The loading screen (UI + its camera) came down on
        // arrival — nothing with the marker survives InGame.
        let mut screen_query = app
            .world_mut()
            .query_filtered::<Entity, With<LoadingScreen>>();
        assert_eq!(
            screen_query.iter(app.world()).count(),
            0,
            "loading screen survived InGame entry"
        );
        // The game camera spawned (the OnEnter(InGame) chain ran).
        let mut cam_query = app.world_mut().query::<&scene::PlayerCamera>();
        assert_eq!(cam_query.iter(app.world()).count(), 1);
    }

    #[test]
    fn door_trigger_radius_and_height() {
        let trigger = bevy::math::Vec3::new(10.0, 0.0, 5.0);
        // Feet on the trigger: inside.
        assert!(inside_door_trigger(
            trigger,
            bevy::math::Vec3::new(10.0, 0.0, 5.0)
        ));
        // 1 m away horizontally: inside (radius 1.2 m).
        assert!(inside_door_trigger(
            trigger,
            bevy::math::Vec3::new(11.0, 0.0, 5.0)
        ));
        // 1.5 m away: outside.
        assert!(!inside_door_trigger(
            trigger,
            bevy::math::Vec3::new(11.5, 0.0, 5.0)
        ));
        // Close horizontally but 2.5 m above: outside (height 2 m).
        assert!(!inside_door_trigger(
            trigger,
            bevy::math::Vec3::new(10.5, 2.5, 5.0)
        ));
        // The real arrival geometry (door_pair): the exterior
        // door ref at Skyrim (23320.5, -44530.2, -8.3) and the
        // XTEL spawn at (23259.8, -44476.2, -8.3) — the player
        // arrives INSIDE the trigger, which is why the transition
        // needs its grace period + edge detection (arriving must
        // not bounce straight back through the door).
        let door = coords::to_bevy_position([23320.5, -44530.2, -8.3]);
        let spawn = coords::to_bevy_position([23259.8, -44476.2, -8.3]);
        assert!(inside_door_trigger(door, spawn));
    }

    #[test]
    fn parse_hex_list_accepts_bare_and_prefixed_hex() {
        assert_eq!(
            parse_hex_list("12eb7,12e49").unwrap(),
            vec![0x12eb7, 0x12e49]
        );
        assert_eq!(
            parse_hex_list("0x12eb7, 0x12e49").unwrap(),
            vec![0x12eb7, 0x12e49]
        );
        assert_eq!(parse_hex_list("f").unwrap(), vec![0x0f]);
        assert!(parse_hex_list("12eb7,zz").is_err());
        assert!(parse_hex_list("").is_err());
    }
}
