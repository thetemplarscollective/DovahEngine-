//! Android data resolution + no-data message screen (M8).
//!
//! The APK must not bundle any Bethesda data, so on Android the game
//! files come from the phone itself. [`find_esm`] looks for
//! `Skyrim.esm` in the places Dylan can put it without a PC:
//!
//! 1. the Downloads folder (downloaded from his own Drive in the
//!    Drive app — one tap, no PC), readable once the app is granted
//!    "All files access" in Settings;
//! 2. the app's external files folder, for a file manager that can
//!    reach `Android/data/`;
//! 3. the app's internal files folder (for completeness — nothing
//!    Dylan can reach without adb, but free to check).
//!
//! The data folder for meshes/textures is the folder the ESM was
//! found in, so BSAs downloaded next to it resolve the same way as
//! on desktop.
//!
//! When the file is missing or unreadable, [`run_message_app`] runs
//! a minimal Bevy app whose whole screen tells Dylan exactly what
//! to do — the app never fails silently to a black screen.
//!
//! Route (a) from the M8 brief — downloading the ESM from Drive
//! inside the app — was rejected: Drive serves 100 MB+ files behind
//! a virus-scan confirmation page, so an in-app download needs
//! cookie handling (or a Drive API key) and breaks whenever Google
//! changes the flow. The Drive app on the phone already handles the
//! download properly, so the APK reads the file after Dylan
//! downloads it instead.
//!
//! M8.1: immersive fullscreen. The M8 playtest showed the Android
//! status bar overlapping the HEALTH bar and the gesture bar over
//! the quick slots. winit's Android `set_fullscreen` is a no-op
//! stub and cargo-apk exposes no theme setting, so this module
//! does it directly with JNI calls, ALL posted to the Java UI
//! thread: `Window.setFlags` for the status bar and
//! `View.setSystemUiVisibility` with the immersive-sticky flags
//! for the navigation bar. Re-applied whenever the window regains
//! focus — Android drops the flags on focus changes.
//!
//! v0.2.1: v0.2.0 called `AndroidApp::set_window_flags` — an NDK
//! call that touches the Activity's Window — directly from the
//! Rust thread. Once the window is attached, `Window.setFlags`
//! reaches `ViewRootImpl`, which throws
//! `CalledFromWrongThreadException` off the UI thread; the
//! exception stays pending on the Rust thread and ART aborts the
//! process at the next JNI call. That was the v0.2.0 crash on
//! launch ("this app has a bug", before any frame). No window
//! call ever runs on the Rust thread now — see
//! [`apply_immersive_mode`].

// Everything here is only called from the `#[cfg(target_os =
// "android")]` branch in `lib.rs`, so on desktop it would all read
// as dead code. The allow is desktop-only: on Android, genuinely
// dead code in this module still warns.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use bevy::app::{App, Startup, Update};
use bevy::camera::ClearColor;
use bevy::color::Color;
use bevy::ecs::prelude::*;
use bevy::text::{FontSize, TextColor, TextFont};
use bevy::ui::widget::Text;
use bevy::ui::{AlignItems, FlexDirection, JustifyContent, Node, PositionType, Val};
// Android-only (the immersive focus system); gated so desktop
// builds don't warn on unused imports.
#[cfg(target_os = "android")]
use bevy::window::WindowFocused;
use std::fs::File;
use std::path::PathBuf;

/// The plugin file the test needs.
pub const ESM_FILE_NAME: &str = "Skyrim.esm";

/// Every game file the M8.1 build reads from Downloads, in the
/// order Dylan downloads them: (file name, size shown to Dylan).
/// The ESM is required; the BSAs are what turn placeholder boxes
/// into real meshes and textures. The provider opens every `*.bsa`
/// in the folder, so the BSA names only matter for the download
/// list — but they are the real archive names, so list them
/// exactly.
pub const DOWNLOADS_FILES: &[(&str, &str)] = &[
    ("Skyrim.esm", "238 MB"),
    ("Skyrim - Meshes0.bsa", "1.1 GB"),
    ("Skyrim - Meshes1.bsa", "356 MB"),
    ("Skyrim - Textures0.bsa", "625 MB"),
    ("Skyrim - Textures1.bsa", "1.4 GB"),
];

/// Total size of [`DOWNLOADS_FILES`], as shown to Dylan
/// (3,940,113,120 bytes).
pub const DOWNLOADS_TOTAL: &str = "about 3.7 GB";

/// Flags for `View.setSystemUiVisibility`:
/// SYSTEM_UI_FLAG_IMMERSIVE_STICKY (0x1000) | FULLSCREEN (0x4) |
/// HIDE_NAVIGATION (0x2) | LAYOUT_STABLE (0x100) |
/// LAYOUT_HIDE_NAVIGATION (0x200) | LAYOUT_FULLSCREEN (0x400).
/// `setSystemUiVisibility` is deprecated since API 30 (replaced by
/// WindowInsetsController) but still works on current Android,
/// and the flags form is one call instead of a controller dance.
const IMMERSIVE_UI_FLAGS: i32 = 0x1000 | 0x4 | 0x2 | 0x100 | 0x200 | 0x400;

/// `WindowManager.LayoutParams.FLAG_FULLSCREEN` (0x400): hides the
/// status bar. Applied via `Window.setFlags` on the Java UI thread
/// (see [`apply_immersive_mode`]) — never via the NDK
/// `set_window_flags` call from the Rust thread (v0.2.0 crash).
const FLAG_FULLSCREEN: i32 = 0x400;

/// Hide the status bar and the navigation bar (M8.1, Android
/// only). Safe to call repeatedly; failures are logged, never
/// fatal — the app must not crash over UI chrome.
///
/// v0.2.1: EVERYTHING runs on the Java UI thread. v0.2.0 called
/// `AndroidApp::set_window_flags` — an NDK call that touches the
/// Activity's Window — directly from the Rust thread. Once the
/// window is attached (it is, by the time the app starts),
/// `Window.setFlags` reaches `ViewRootImpl`, which throws
/// `CalledFromWrongThreadException` off the UI thread; the
/// exception stays pending on the Rust thread and ART aborts the
/// process at the next JNI call — the v0.2.0 crash on launch
/// ("this app has a bug", before any frame). The status-bar flag
/// now goes through the same UI-thread JNI call as the
/// navigation-bar flags, so no window call ever runs on the Rust
/// thread.
#[cfg(target_os = "android")]
pub fn apply_immersive_mode() {
    // v0.2.2: diagnostic kill-switch — a file named
    // `dovahengine-no-immersive` in Downloads skips the JNI path
    // entirely (and says so in the log). The v0.2.1 crash turned
    // out not to be the JNI code (the message screen uses it and
    // runs clean), but the flag stays as a cheap way to rule it
    // out on the phone.
    if crate::startup_log::no_immersive_flag() {
        crate::startup_log::log("immersive mode skipped (no-immersive flag file present)");
        return;
    }
    let Some(app) = bevy::android::ANDROID_APP.get() else {
        crate::startup_log::log("immersive mode skipped (ANDROID_APP not set)");
        return;
    };
    // The closure runs on the Java UI thread via the activity's
    // main looper (async: queued, not blocking). android-activity
    // wraps the looper callback in `abort_on_panic`, so a panic
    // inside the closure would kill the process — `catch_unwind`
    // keeps a bug in immersive mode from ever crashing the app.
    // The closure gets its own clone: `run_on_java_main_thread`
    // borrows `app`, so the closure cannot move the same binding.
    let app_for_ui_thread = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            set_immersive_sticky(&app_for_ui_thread);
        }));
    }));
}

/// The JNI half of [`apply_immersive_mode`]: runs on the Java UI
/// thread. Calls `activity.getWindow().setFlags(FLAG_FULLSCREEN,
/// FLAG_FULLSCREEN)` (status bar — this replaces the v0.2.0 NDK
/// call that crashed) and `getWindow().getDecorView()`
/// `.setSystemUiVisibility(FLAGS)` (navigation bar, immersive
/// sticky).
///
/// Hardening (v0.2.1): jni-rs reports a thrown Java exception as
/// an error but leaves it pending — if it is never cleared, the
/// process aborts the next time the UI thread runs Java code. So
/// any pending exception is described (logcat) and cleared before
/// this returns, on the error path as well as the success path.
/// Null VM/activity/window/decor bail out with a log instead of
/// calling into a null object.
#[cfg(target_os = "android")]
fn set_immersive_sticky(app: &android_activity::AndroidApp) {
    use jni::{objects::JObject, JavaVM};
    let vm_ptr = app.vm_as_ptr();
    let activity_ptr = app.activity_as_ptr();
    if vm_ptr.is_null() || activity_ptr.is_null() {
        eprintln!("warning: immersive mode skipped (null VM or activity pointer)");
        return;
    }
    let result: Result<(), jni::errors::Error> = (|| {
        let vm = unsafe { JavaVM::from_raw(vm_ptr as *mut jni::sys::JavaVM) }?;
        let mut env = vm.attach_current_thread()?;
        // `activity_as_ptr` is the ANativeActivity's global
        // activity reference — wrapping it does not take ownership,
        // and it must not be deleted.
        let outcome = (|| {
            let activity = unsafe { JObject::from_raw(activity_ptr as jni::sys::jobject) };
            let window = env
                .call_method(&activity, "getWindow", "()Landroid/view/Window;", &[])?
                .l()?;
            if window.is_null() {
                return Err(jni::errors::Error::NullPtr("getWindow() returned null"));
            }
            // Status bar: Window.setFlags(int flags, int mask).
            env.call_method(
                &window,
                "setFlags",
                "(II)V",
                &[FLAG_FULLSCREEN.into(), FLAG_FULLSCREEN.into()],
            )?;
            let decor = env
                .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])?
                .l()?;
            if decor.is_null() {
                return Err(jni::errors::Error::NullPtr("getDecorView() returned null"));
            }
            env.call_method(
                &decor,
                "setSystemUiVisibility",
                "(I)V",
                &[IMMERSIVE_UI_FLAGS.into()],
            )?;
            Ok(())
        })();
        // A pending Java exception must never escape to the looper:
        // describe it for logcat, then clear it.
        if let Ok(true) = env.exception_check() {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        outcome
    })();
    match result {
        Ok(()) => crate::startup_log::log("immersive mode applied"),
        Err(e) => {
            crate::startup_log::log(&format!("immersive mode failed: {e:?}"));
            eprintln!("warning: immersive mode failed: {e:?}");
        }
    }
}

/// Re-apply immersive mode whenever the window regains focus
/// (Android drops the system-UI flags on focus changes). Android
/// only; added to the main app and the message app.
#[cfg(target_os = "android")]
pub fn reapply_immersive_on_focus(mut events: MessageReader<WindowFocused>) {
    for event in events.read() {
        if event.focused {
            apply_immersive_mode();
        }
    }
}

/// The APK's package name (also the `Android/data/` folder name).
pub const PACKAGE_NAME: &str = "com.dovahengine.inntest";

/// Where [`find_esm`] looks, in order.
pub fn candidate_esm_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/storage/emulated/0/Download").join(ESM_FILE_NAME),
        PathBuf::from("/sdcard/Download").join(ESM_FILE_NAME),
        PathBuf::from("/storage/emulated/0/Android/data")
            .join(PACKAGE_NAME)
            .join("files")
            .join(ESM_FILE_NAME),
        PathBuf::from("/data/data")
            .join(PACKAGE_NAME)
            .join("files")
            .join(ESM_FILE_NAME),
    ]
}

/// Outcome of [`find_esm`].
pub enum EsmSearch {
    /// The ESM exists and opens. `data_dir` is the folder it sits
    /// in — BSAs next to it resolve as the data folder.
    Found { esm: PathBuf, data_dir: PathBuf },
    /// No candidate path exists.
    NotFound { checked: Vec<PathBuf> },
    /// The file exists but cannot be opened (no file permission).
    ExistsButUnreadable { path: PathBuf },
}

/// Find a readable `Skyrim.esm` on the phone.
pub fn find_esm() -> EsmSearch {
    let checked = candidate_esm_paths();
    for path in &checked {
        if path.is_file() {
            match File::open(path) {
                Ok(_) => {
                    let data_dir = path
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from("/"));
                    return EsmSearch::Found {
                        esm: path.clone(),
                        data_dir,
                    };
                }
                Err(_) => {
                    return EsmSearch::ExistsButUnreadable { path: path.clone() };
                }
            }
        }
    }
    EsmSearch::NotFound { checked }
}

/// Why the message screen is showing.
#[derive(Resource)]
pub enum MissingReason {
    NotFound { checked: Vec<PathBuf> },
    Unreadable { path: PathBuf },
}

impl MissingReason {
    fn title(&self) -> &'static str {
        match self {
            MissingReason::NotFound { .. } => "Skyrim.esm not found",
            MissingReason::Unreadable { .. } => "Skyrim.esm cannot be read",
        }
    }

    fn body(&self) -> String {
        match self {
            MissingReason::NotFound { checked } => {
                let mut text = String::from(
                    "Dovahengine needs your Skyrim data files to load the \
                     inn. The main file is not on this phone yet.\n\n\
                     1. Open Google Drive and download these files to this \
                     phone (they land in Downloads):\n",
                );
                for (name, size) in DOWNLOADS_FILES {
                    text.push_str(&format!("   - {name} ({size})\n"));
                }
                text.push_str(&format!(
                    "   Total: {DOWNLOADS_TOTAL}, so use Wi-Fi.\n\n\
                     2. Allow file access: Settings > Apps > Dovahengine \
                     Inn Test > Permissions > Files and media > \
                     Allow management of all files.\n\n\
                     3. Reopen this app.\n\n\
                     Looked for Skyrim.esm at:\n",
                ));
                for path in checked {
                    text.push_str(&format!("- {}\n", path.display()));
                }
                text
            }
            MissingReason::Unreadable { path } => format!(
                "Found Skyrim.esm, but the app cannot read it.\n\n\
                 Allow file access: Settings > Apps > Dovahengine Inn Test \
                 > Permissions > Files and media > \
                 Allow management of all files.\n\n\
                 Then reopen this app.\n\n\
                 File: {}",
                path.display()
            ),
        }
    }
}

/// Run a minimal Bevy app that shows the message screen and nothing
/// else. Blocks until the app exits.
pub fn run_message_app(reason: MissingReason) {
    // Fullscreen even on the message screen (M8.1).
    #[cfg(target_os = "android")]
    apply_immersive_mode();
    crate::startup_log::log("message app: app.run() starting");
    let mut app = App::new();
    app.insert_resource(ClearColor(Color::srgb(0.05, 0.05, 0.07)))
        .insert_resource(reason)
        .add_plugins(bevy::DefaultPlugins)
        .add_systems(Startup, spawn_message_ui)
        // v0.2.2: first-frame marker (same as the main app).
        .add_systems(Update, crate::log_frame_tick_system);
    #[cfg(target_os = "android")]
    app.add_systems(Update, reapply_immersive_on_focus);
    app.run();
}

fn spawn_message_ui(mut commands: Commands, reason: Res<MissingReason>) {
    commands.spawn(bevy::camera::Camera2d);
    commands
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::FlexStart,
            padding: bevy::ui::UiRect::all(Val::Px(48.0)),
            ..Default::default()
        })
        .with_children(|parent| {
            parent.spawn((
                Text::new("Dovahengine — Inn Test"),
                TextFont {
                    font_size: FontSize::Px(34.0),
                    ..Default::default()
                },
                TextColor(Color::srgb(0.85, 0.75, 0.45)),
            ));
            parent.spawn((
                Text::new(reason.title()),
                TextFont {
                    font_size: FontSize::Px(28.0),
                    ..Default::default()
                },
                TextColor(Color::WHITE),
                Node {
                    margin: bevy::ui::UiRect::top(Val::Px(24.0)),
                    ..Default::default()
                },
            ));
            parent.spawn((
                Text::new(reason.body()),
                TextFont {
                    font_size: FontSize::Px(22.0),
                    ..Default::default()
                },
                TextColor(Color::srgb(0.85, 0.85, 0.85)),
                Node {
                    width: Val::Percent(92.0),
                    margin: bevy::ui::UiRect::top(Val::Px(16.0)),
                    position_type: PositionType::Relative,
                    ..Default::default()
                },
            ));
            // v0.2.2: crash viewer — when a previous run left a
            // log behind, show its tail on the message screen so
            // the last thing the app did is visible without
            // opening the file. The full log is in Downloads as
            // `dovahengine-startup-prev.log`.
            if let Some(tail) = crate::startup_log::prev_log_tail(8) {
                parent.spawn((
                    Text::new(format!(
                        "\nLast run ended here (full log: Downloads/dovahengine-startup-prev.log):\n{tail}"
                    )),
                    TextFont {
                        font_size: FontSize::Px(18.0),
                        ..Default::default()
                    },
                    TextColor(Color::srgb(0.95, 0.75, 0.55)),
                    Node {
                        width: Val::Percent(92.0),
                        margin: bevy::ui::UiRect::top(Val::Px(16.0)),
                        position_type: PositionType::Relative,
                        ..Default::default()
                    },
                ));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_all_point_at_the_esm() {
        let paths = candidate_esm_paths();
        assert!(!paths.is_empty());
        for path in &paths {
            assert_eq!(path.file_name().unwrap().to_str().unwrap(), ESM_FILE_NAME);
        }
    }

    #[test]
    fn downloads_is_first() {
        let paths = candidate_esm_paths();
        assert!(paths[0].starts_with("/storage/emulated/0/Download"));
    }

    #[test]
    fn immersive_flags_are_the_android_constants() {
        // SYSTEM_UI_FLAG_IMMERSIVE_STICKY (0x1000) | FULLSCREEN
        // (0x4) | HIDE_NAVIGATION (0x2) | LAYOUT_STABLE (0x100) |
        // LAYOUT_HIDE_NAVIGATION (0x200) | LAYOUT_FULLSCREEN
        // (0x400) — a wrong bit here silently shows a bar, so pin
        // the value.
        assert_eq!(
            IMMERSIVE_UI_FLAGS,
            0x1000 | 0x4 | 0x2 | 0x100 | 0x200 | 0x400
        );
        assert_eq!(IMMERSIVE_UI_FLAGS, 0x1706);
        // WindowManager.LayoutParams.FLAG_FULLSCREEN.
        assert_eq!(FLAG_FULLSCREEN, 0x400);
    }
}
