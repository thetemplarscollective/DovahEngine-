//! `ruffle-harness` binary (M1): load `hudmenu.gfx`, drive it with
//! stub game data, capture frames as PNGs.
//!
//! Usage: `ruffle-harness [swf] [fontlib] [out-dir] [total-frames] [mode]`
//!
//! * `swf` — default `interface-extract/study/hudmenu.gfx`
//! * `fontlib` — default
//!   `interface-extract/tools/ruffle-test/gfxfontlib.swf`
//! * `out-dir` — default `screenshots`
//! * `total-frames` — default 60. Longer runs (e.g. 250) probe
//!   slow timeline animations (the meter/location fade-ins span
//!   170–200 SWF frames).
//! * `mode` — `demo` (default), `probe`, `loc`, `jump`, `force`, `upd`,
//!   or `inventory`: `probe` jumps the meter/location clips to mid-timeline frames
//!   after the drive, to tell a timeline-advancement fault from a
//!   content fault; `loc` drives ONLY `SetLocationName` (isolates
//!   the location name); `jump` runs `InitExtensions` + the probe
//!   jumps with no `Set*` calls (tests whether the probe itself
//!   works — the DEFAULT "Location" text should appear at frame
//!   85 if the jumps land); `force` drives FORCED percents
//!   (25/50/90), bypassing the `onEnterFrame` → `Update()` tween;
//!   `upd` drives unforced percents then calls each Meter's
//!   `Update()` directly 10 times (tests whether `Update` works
//!   when `onEnterFrame` is out of the loop); `inventory` drives
//!   the inventory menu with stub categories, items, and player
//!   info (M3 — use with inventorymenu.swf).
//!
//! All paths are relative to the working directory: run from the
//! workspace root (`~/workspace/dovahengine`).
//!
//! Outputs in `out-dir`:
//!
//! * `ruffle-m1-hud-nodata.png` — the HUD after settle frames, with
//!   NO game data. Should match the study's `shot-hud-fontlib.png`
//!   (real compass, "Location", subtitle lines, real font).
//! * `ruffle-m1-hud.png` — the HUD after the stub-data drive:
//!   meters visible, location "Riverwood", crosshair target
//!   "Delphine". This is the M1 proof artifact.
//!
//! The run also prints build time, per-frame timing (mean/p95/max
//! over ~60 frames), and the SWF→game call log.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use ruffle_harness::{summarize, Harness, HarnessConfig, DRIVE_FRAMES, SETTLE_FRAMES};

/// Total frames stepped by default, so the timing summary covers a
/// real run (~60 frames at 30 fps = ~2 s of HUD). Overridable with
/// the 4th CLI arg for long timeline probes.
const TOTAL_FRAMES: u32 = 60;

/// Capture a sequence frame every N steps, named by absolute frame
/// number — the meter/location fade-ins are timed animations, and the
/// capture point matters.
const SEQ_EVERY: u32 = 15;

/// Initialize Ruffle's `tracing` output when `RUST_LOG` is set (M3
/// diagnostic). Without a subscriber, Ruffle's loader logs
/// (`Loading imported movie`, preload results, fetch errors) go
/// nowhere — the harness needs them to see why a menu's import
/// chain stalls. The level is the value itself (`debug`, `info`,
/// ...); a `target=level` form takes the level after the last `=`,
/// and anything unparsable falls back to DEBUG. Off by default so
/// normal runs stay clean, and logs go to stderr so stdout keeps
/// the harness's own output.
fn init_tracing() {
    let Ok(level) = std::env::var("RUST_LOG") else {
        return;
    };
    let level = level.rsplit('=').next().unwrap_or("debug");
    let max_level = match level.to_ascii_lowercase().as_str() {
        "trace" => tracing::Level::TRACE,
        "info" => tracing::Level::INFO,
        "warn" => tracing::Level::WARN,
        "error" => tracing::Level::ERROR,
        _ => tracing::Level::DEBUG,
    };
    let _ = tracing_subscriber::fmt()
        .with_max_level(max_level)
        .with_writer(std::io::stderr)
        .try_init();
}

fn run() -> Result<(), String> {
    init_tracing();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let swf = args
        .first()
        .cloned()
        .unwrap_or_else(|| "interface-extract/study/hudmenu.gfx".to_string());
    let fontlib = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "interface-extract/tools/ruffle-test/gfxfontlib.swf".to_string());
    let out_dir = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "screenshots".to_string());
    let total_frames: u32 = args
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(TOTAL_FRAMES)
        .max(SETTLE_FRAMES + DRIVE_FRAMES + 1);
    let mode: &str = args.get(4).map(|s| s.as_str()).unwrap_or("demo");

    let config = HarnessConfig::new(PathBuf::from(&swf), PathBuf::from(&fontlib));
    println!("ruffle-harness M1: swf={swf} fontlib={fontlib} out={out_dir}");

    let started = Instant::now();
    let mut harness = Harness::new(&config).map_err(|e| e.to_string())?;
    println!("player built in {:.2}s", started.elapsed().as_secs_f64());

    let mut timings = Vec::with_capacity(TOTAL_FRAMES as usize);

    // Settle: frame-1 scripts run, the fontlib import completes.
    for _ in 0..SETTLE_FRAMES {
        timings.push(harness.step());
    }
    let nodata_path = PathBuf::from(&out_dir).join("ruffle-m1-hud-nodata.png");
    match harness.capture() {
        Some(image) => {
            image.save(&nodata_path).map_err(|e| e.to_string())?;
            println!("no-data frame: {}", nodata_path.display());
        }
        None => return Err("no frame captured after settle frames".to_string()),
    }

    // Drive with stub game data, then let the meter anims play.
    // Capture a sequence during the whole run: the meter/location
    // fade-ins are timed animations, and the capture point matters.
    //
    // In probe/jump modes, verify path resolution FIRST: a jump's
    // return value cannot tell a landed `gotoAndStop` (returns
    // Undefined) from a missing path (also Undefined), but
    // `getDepth` returns the clip's depth number on success.
    if mode == "probe" || mode == "jump" {
        for path in Harness::PROBE_CLIP_PATHS {
            let depth = harness.clip_depth(path);
            println!("  depth {path} = {depth:?}");
        }
    }
    match mode {
        "probe" => {
            println!("mode: probe (jump meter/location clips mid-timeline)");
            harness.drive_hud_probe();
        }
        "loc" => {
            println!("mode: loc (SetLocationName only)");
            harness.drive_hud_loc();
        }
        "jump" => {
            println!("mode: jump (InitExtensions + probe jumps only)");
            harness.drive_hud_jump();
        }
        "force" => {
            println!("mode: force (forced percents 25/50/90)");
            harness.drive_hud_force();
        }
        "upd" => {
            println!("mode: upd (unforced percents + direct Update calls)");
            harness.drive_hud_upd();
        }
        "inventory" => {
            println!("mode: inventory (stub categories + items + player info)");
            let injected = harness.drive_inventory_demo();
            println!("  entryList injection landed: {injected}");
        }
        _ => {
            harness.drive_hud_demo();
        }
    }
    // State right after the drive (before any frame steps): did the
    // jumps land, did SetLocationName restart the location clip?
    // Inventory mode dumps the inventory clips instead of the HUD's.
    let state_paths: &[&str] = if mode == "inventory" {
        &Harness::INV_STATE_PATHS
    } else {
        &Harness::STATE_PATHS
    };
    println!("clip state after drive:");
    for path in state_paths {
        println!("  {}", harness.clip_state(path));
    }
    let seq_dir = PathBuf::from(&out_dir).join("ruffle-m1-seq");
    std::fs::create_dir_all(&seq_dir).map_err(|e| e.to_string())?;
    let maybe_capture = |harness: &mut Harness| -> Result<(), String> {
        if harness.frames_stepped().is_multiple_of(SEQ_EVERY) {
            if let Some(image) = harness.capture() {
                image
                    .save(seq_dir.join(format!("seq-{:03}.png", harness.frames_stepped())))
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    };
    for _ in 0..DRIVE_FRAMES {
        timings.push(harness.step());
        maybe_capture(&mut harness)?;
    }
    println!("drive sequence: {}", seq_dir.display());
    // State after the drive frames (at capture time): where did the
    // timelines end up — playing, parked, visible?
    println!("clip state after drive frames:");
    for path in state_paths {
        println!("  {}", harness.clip_state(path));
    }
    let hud_path = PathBuf::from(&out_dir).join("ruffle-m1-hud.png");
    match harness.capture() {
        Some(image) => {
            image.save(&hud_path).map_err(|e| e.to_string())?;
            println!("driven frame: {}", hud_path.display());
        }
        None => return Err("no frame captured after drive frames".to_string()),
    }

    // Keep stepping to total_frames for a real timing sample
    // (meters fade out on the SWF's own timer during these).
    while harness.frames_stepped() < total_frames {
        timings.push(harness.step());
        maybe_capture(&mut harness)?;
    }

    let summary = summarize(&timings);
    let steady = summarize(&timings[SETTLE_FRAMES as usize..]);
    println!(
        "timing all {} frames: run mean {:.2}ms p95 {:.2}ms max {:.2}ms | render mean {:.2}ms max {:.2}ms",
        summary.frames,
        summary.run_mean_ms,
        summary.run_p95_ms,
        summary.run_max_ms,
        summary.render_mean_ms,
        summary.render_max_ms
    );
    println!(
        "timing steady-state {} frames: run mean {:.2}ms p95 {:.2}ms max {:.2}ms | render mean {:.2}ms max {:.2}ms",
        steady.frames,
        steady.run_mean_ms,
        steady.run_p95_ms,
        steady.run_max_ms,
        steady.render_mean_ms,
        steady.render_max_ms
    );

    let state = harness.provider_state();
    let state = state.lock().expect("provider state lock");
    println!(
        "provider: {} SWF→game calls, {} responds sent, callbacks: {:?}",
        state.calls.len(),
        state.responds_sent,
        state.callbacks_available
    );
    for call in &state.calls {
        println!("  {}({} args) {:?}", call.name, call.args.len(), call.args);
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ruffle-harness: error: {e}");
            ExitCode::FAILURE
        }
    }
}
