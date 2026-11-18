//! Ruffle harness (M1): run Skyrim's real Flash menus offscreen with
//! stub game data, and capture the rendered frames as PNGs.
//!
//! See `docs/flash-interface-feasibility.md` §5/M1 for the plan this
//! crate implements. The three modules:
//!
//! * [`navigator`]: serves the synthesized `gfxfontlib.swf` from
//!   memory; other fetches fall back to disk.
//! * [`provider`]: logs every SWF→game call; answers
//!   `GetButtonFromUserEvent` synchronously via `"respond"`.
//! * [`runner`]: builds the player on its own offscreen wgpu device,
//!   drives frames, captures screenshots.

pub mod fontlib;
pub mod navigator;
pub mod provider;
pub mod runner;

pub use fontlib::{synthesize_fontlib, FontlibError, FONTLIB_EXPORTS};
pub use navigator::{is_fontlib_url, MemoryNavigatorBackend};
pub use provider::{CallRecord, ProviderState, StubProvider};
/// The value type for GameDelegate calls (`Harness::call` takes
/// `Vec<Value>`). Re-exported so the engine does not need a direct
/// `ruffle_core` dependency to drive the player (M4).
pub use ruffle_core::external::Value;
pub use runner::{
    summarize, FrameTiming, Harness, HarnessConfig, HarnessError, SharedWgpu, TimingSummary,
    DEFAULT_FRAME_RATE, DEFAULT_HEIGHT, DEFAULT_WIDTH, DRIVE_FRAMES, SETTLE_FRAMES,
};
