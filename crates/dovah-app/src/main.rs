//! Desktop binary: a thin wrapper over the library target.
//!
//! The app itself lives in `lib.rs` (M8: `cargo-apk` needs a
//! `cdylib` library to build the Android APK, and the `#[bevy_main]`
//! entry point lives there). This binary just calls it.

// Exit status is handled inside `dovah_app::main`: it exits the
// process directly on failure (see lib.rs).
fn main() {
    dovah_app::main();
}
