//! Startup-stage log (v0.2.2): a crash-safe diary of what the app
//! did, in a file Dylan can reach and send to Jarvis.
//!
//! v0.2.0 and v0.2.1 both crashed on Dylan's phone with no logcat
//! available to Jarvis, so the cause was guessed twice (wrongly the
//! second time). This module ends the guessing: every startup stage
//! appends one timestamped line to
//! `Downloads/dovahengine-startup.log` — the same folder as the
//! game files, which the app can already read — opening in append
//! mode and CLOSING the file after every line, so a crash loses at
//! most the line being written. The panic hook (installed before
//! any Bevy/Android init) appends the panic message, location and
//! a backtrace the same way.
//!
//! On launch the previous run's log is rotated to
//! `dovahengine-startup-prev.log`, so a crashed run's diary
//! survives the next launch, and the message screen shows its tail.
//!
//! The log directory is the first writable one of: Downloads,
//! the app's external files folder, the app's internal files
//! folder (Android); the system temp dir on desktop. If none is
//! writable the logger silently no-ops — logging must never crash
//! the app it is diagnosing.
//!
//! All functions are thread-safe (the ESM parse runs on a worker
//! thread and logs from there) and never panic.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// The log file name (current run).
pub const LOG_FILE_NAME: &str = "dovahengine-startup.log";
/// The rotated previous run's log.
pub const PREV_LOG_FILE_NAME: &str = "dovahengine-startup-prev.log";
/// Flag file: when present in Downloads, immersive fullscreen is
/// skipped (diagnostic kill-switch for the JNI path).
/// Android-only in practice: the only caller is the Android
/// immersive path, so desktop builds would warn it is unused.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub const NO_IMMERSIVE_FLAG_NAME: &str = "dovahengine-no-immersive";

/// Package name, for the app-specific fallback directories.
/// Android-only: the desktop candidate list is just the temp dir.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const PACKAGE_NAME: &str = "com.dovahengine.inntest";

/// Logger state: the chosen log path (None until [`init`], or when
/// no directory was writable) and the start instant for timestamps.
struct State {
    path: Option<PathBuf>,
    prev_path: Option<PathBuf>,
    start: Instant,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| {
        Mutex::new(State {
            path: None,
            prev_path: None,
            start: Instant::now(),
        })
    })
}

/// Candidate log directories, best first. Downloads is first
/// because Dylan can reach it; the app-specific folders need no
/// file permission, so the log still lands when permission is
/// denied (the message-screen path).
fn candidate_dirs() -> Vec<PathBuf> {
    #[cfg(target_os = "android")]
    {
        vec![
            PathBuf::from("/storage/emulated/0/Download"),
            PathBuf::from("/sdcard/Download"),
            PathBuf::from("/storage/emulated/0/Android/data")
                .join(PACKAGE_NAME)
                .join("files"),
            PathBuf::from("/data/data").join(PACKAGE_NAME).join("files"),
        ]
    }
    #[cfg(not(target_os = "android"))]
    {
        vec![std::env::temp_dir()]
    }
}

/// Initialize the logger: pick the first writable directory,
/// rotate any previous log to `*-prev.log`, and write the header
/// line. Call FIRST in `main()`, before any Bevy/Android init.
/// Returns the chosen log path (None when no directory worked).
pub fn init() -> Option<PathBuf> {
    let mut chosen: Option<(PathBuf, bool)> = None;
    for dir in candidate_dirs() {
        let _ = fs::create_dir_all(&dir);
        let path = dir.join(LOG_FILE_NAME);
        // Did a previous run leave a log here? Check BEFORE the
        // probe below creates the file — otherwise every fresh
        // start "rotates" the empty file it just created.
        let existed = path.is_file();
        // Probe writability by opening in append mode.
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(_) => {
                chosen = Some((path, existed));
                break;
            }
            Err(_) => continue,
        }
    }
    let Some((path, existed)) = chosen else {
        eprintln!("startup log: no writable log directory, logging disabled");
        return None;
    };
    // Rotate the previous run's log so a crashed run's diary
    // survives this launch. Best effort: a stale prev file is
    // replaced, a failed rename is ignored (the new log still
    // starts fresh below). Only when a previous log existed.
    let prev_path = path.with_file_name(PREV_LOG_FILE_NAME);
    let rotated = if existed {
        let _ = fs::remove_file(&prev_path);
        fs::rename(&path, &prev_path).is_ok()
    } else {
        false
    };
    if let Ok(mut guard) = state().lock() {
        guard.path = Some(path.clone());
        guard.prev_path = Some(prev_path.clone());
        guard.start = Instant::now();
    }
    write_line(&format!(
        "dovahengine startup log (v{}) — previous run {}",
        env!("CARGO_PKG_VERSION"),
        if rotated {
            "rotated to prev log"
        } else {
            "none"
        }
    ));
    Some(path)
}

/// Append one timestamped line to the log (open, append, close —
/// crash-safe) and mirror it to stderr (logcat). Never panics;
/// silently drops the line when the logger is not initialized or
/// the write fails.
pub fn log(message: &str) {
    let elapsed = state()
        .lock()
        .map(|g| g.start.elapsed())
        .unwrap_or_default();
    write_line(&format!("[+{:.3}s] {message}", elapsed.as_secs_f32()));
}

/// Like [`log`], with the process's current resident memory
/// appended (`RSS ~123 MB` on Linux via `/proc/self/statm`).
/// Cheap enough to call at every stage boundary.
pub fn log_rss(message: &str) {
    match rss_mb() {
        Some(mb) => log(&format!("{message} (RSS ~{mb} MB)")),
        None => log(message),
    }
}

/// Resident set size in MB, from `/proc/self/statm` (Linux and
/// Android). `None` anywhere else, or when the file cannot be read.
pub fn rss_mb() -> Option<u64> {
    let text = fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: u64 = text.split_whitespace().nth(1)?.parse().ok()?;
    // Page size is 4 KiB on every Android device and on x86_64
    // Linux; the value is an estimate for the log, not accounting.
    Some(resident_pages * 4 / 1024)
}

/// Frame counter for the first-frame marker. Call once per Update
/// tick: logs on tick 1 ("first update tick") and tick 2 ("second
/// update tick — first frame rendered", since Update 2 only runs
/// after frame 1 has rendered). Silent after that. Shared by the
/// main app and the message app; the count resets per process.
static FRAME_TICK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Log the first-frame marker (see [`FRAME_TICK`]).
pub fn log_frame_tick() {
    let tick = FRAME_TICK.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    match tick {
        1 => log_rss("first update tick"),
        2 => log("second update tick — first frame rendered"),
        _ => {}
    }
}

/// Whether the immersive-mode kill-switch flag file exists in
/// Downloads. Checked by `android::apply_immersive_mode`.
/// Android-only in practice: the only caller is Android-gated.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn no_immersive_flag() -> bool {
    #[cfg(target_os = "android")]
    {
        PathBuf::from("/storage/emulated/0/Download")
            .join(NO_IMMERSIVE_FLAG_NAME)
            .is_file()
            || PathBuf::from("/sdcard/Download")
                .join(NO_IMMERSIVE_FLAG_NAME)
                .is_file()
    }
    #[cfg(not(target_os = "android"))]
    {
        std::env::temp_dir().join(NO_IMMERSIVE_FLAG_NAME).is_file()
    }
}

/// The last `n` lines of the previous run's log, for the message
/// screen's crash viewer. `None` when there is no previous log.
pub fn prev_log_tail(n: usize) -> Option<String> {
    let prev = state().lock().ok().and_then(|g| g.prev_path.clone())?;
    let text = fs::read_to_string(prev).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    Some(lines[start..].join("\n"))
}

/// Install the panic hook: append the panic message, location and
/// a backtrace to the log, then run the default hook (so logcat
/// still gets the panic). Install FIRST in `main()`, before any
/// Bevy/Android init — a panic during startup is exactly what this
/// is for. The hook itself never panics: logging is best-effort.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic payload".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());
        log(&format!("PANIC: {message} at {location}"));
        // A backtrace is gold when the panic is inside a library;
        // force-capture (no RUST_BACKTRACE on the phone) and cap
        // the length so one panic cannot fill the log.
        let backtrace = std::backtrace::Backtrace::force_capture();
        let text = backtrace.to_string();
        let capped: String = text.chars().take(4096).collect();
        for line in capped.lines().take(40) {
            log(&format!("  bt: {line}"));
        }
        default_hook(info);
    }));
}

/// Write one line (no timestamp — the caller formats it). Opens
/// in append mode and drops the handle: nothing is buffered, so a
/// crash after this returns cannot lose the line. Also mirrors to
/// stderr. Never panics.
fn write_line(line: &str) {
    eprintln!("{line}");
    let path = match state().lock() {
        Ok(guard) => guard.path.clone(),
        Err(_) => return,
    };
    let Some(path) = path else {
        return;
    };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
        // The handle drops here: the line is on disk before this
        // returns (append + close per line — the whole point).
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Serialises the tests that touch the global logger. Unique
    /// temp dirs are not enough: the logger's *pointer* is global,
    /// so one test's `point_logger_at` can steal another test's
    /// `log` calls mid-test (flaky failure seen 2026-11-14:
    /// `log_lines_land_in_the_file` failed when its lines landed
    /// in a sibling test's directory). Each logger test holds this
    /// for its whole body.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn hold_logger_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A fresh temp directory per test (the logger's global state
    /// is shared, so tests that write must not share a path).
    fn test_dir() -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("dovah-startup-log-test-{n}"));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    /// Point the global logger at `dir` directly (bypasses
    /// [`init`]'s candidate list, which is platform-fixed).
    fn point_logger_at(dir: &std::path::Path) {
        if let Ok(mut guard) = state().lock() {
            guard.path = Some(dir.join(LOG_FILE_NAME));
            guard.prev_path = Some(dir.join(PREV_LOG_FILE_NAME));
            guard.start = Instant::now();
        }
    }

    #[test]
    fn log_lines_land_in_the_file() {
        let _guard = hold_logger_lock();
        let dir = test_dir();
        point_logger_at(&dir);
        log("stage one");
        log("stage two");
        let text = fs::read_to_string(dir.join(LOG_FILE_NAME)).unwrap();
        assert!(text.contains("stage one"), "log was:\n{text}");
        assert!(text.contains("stage two"), "log was:\n{text}");
        // Timestamp prefix on every line.
        for line in text.lines() {
            assert!(line.starts_with("[+"), "line had no timestamp: {line}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_keeps_the_previous_run() {
        let _guard = hold_logger_lock();
        let dir = test_dir();
        // A "previous run" log already on disk.
        fs::write(dir.join(LOG_FILE_NAME), "old run line\n").unwrap();
        // init() would rotate it — simulate the rotation the same
        // way init does (rename current to prev, start fresh).
        let prev = dir.join(PREV_LOG_FILE_NAME);
        let _ = fs::remove_file(&prev);
        fs::rename(dir.join(LOG_FILE_NAME), &prev).unwrap();
        point_logger_at(&dir);
        log("new run line");
        let prev_text = fs::read_to_string(&prev).unwrap();
        assert!(prev_text.contains("old run line"));
        let cur_text = fs::read_to_string(dir.join(LOG_FILE_NAME)).unwrap();
        assert!(cur_text.contains("new run line"));
        assert!(!cur_text.contains("old run line"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prev_log_tail_returns_the_last_lines() {
        let _guard = hold_logger_lock();
        let dir = test_dir();
        let mut prev = String::new();
        for i in 0..10 {
            prev.push_str(&format!("line {i}\n"));
        }
        fs::write(dir.join(PREV_LOG_FILE_NAME), prev).unwrap();
        point_logger_at(&dir);
        let tail = prev_log_tail(3).unwrap();
        assert!(tail.contains("line 7"));
        assert!(tail.contains("line 9"));
        assert!(!tail.contains("line 0\n"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rss_reads_on_linux() {
        // /proc/self/statm exists on this machine and on Android.
        let mb = rss_mb();
        assert!(mb.is_some(), "statm unreadable on this platform");
        assert!(mb.unwrap() > 0);
    }
}
