//! Frame-time telemetry for the phone log (v0.4.11 perf work).
//!
//! Dylan's #1 complaint about v0.4.10 is "so framey and laggy",
//! and no frame-time data exists in the phone log — so the cost
//! was unmeasurable. This module closes the gap: a [`FrameStats`]
//! resource accumulates per-frame deltas (main world, InGame
//! only) and [`log_frame_time`] writes one summary line to the
//! startup log every [`PERF_WINDOW_SECS`] seconds.
//!
//! The summary carries what "framey" means: average frame time
//! (+ FPS), the worst frame, and bucket counts of frames over
//! 16.7 / 33.3 / 50 / 100 ms — a stuttery game shows up as a fat
//! tail, not a bad average. No allocation in the hot path: the
//! stats are plain counters, and the only allocation is the one
//! formatted line per window.
//!
//! Caveat: on the desktop screenshot path the schedule runner
//! ticks at 30 Hz, so deltas read ~33 ms whenever the real work
//! is faster — the logger measures max(work, tick period) there,
//! and only work ABOVE 33 ms/frame shows up as a longer average.
//! On the phone (vsync-driven) the deltas are real frame times.
//! Bevy's `Time` also clamps deltas (250 ms default), so the max
//! bucket caps there — a hitch longer than the clamp still lands
//! in the >100 ms bucket, which is the signal that matters.

#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

use bevy::prelude::*;

/// Seconds per telemetry window.
pub const PERF_WINDOW_SECS: f64 = 5.0;

/// Frame-time buckets (seconds): over one 60 fps frame, over one
/// 30 fps frame, over 50 ms, over 100 ms.
const BUCKET_60FPS: f32 = 1.0 / 60.0;
const BUCKET_30FPS: f32 = 1.0 / 30.0;
const BUCKET_50MS: f32 = 0.050;
const BUCKET_100MS: f32 = 0.100;

/// Accumulated frame-time stats for one window. Pure data —
/// [`FrameStats::record`] / [`FrameStats::flush`] are
/// unit-tested without Bevy.
#[derive(Debug, Resource)]
pub struct FrameStats {
    window_start: Instant,
    window_index: u32,
    frames: u32,
    total_secs: f64,
    max_secs: f32,
    over_60fps: u32,
    over_30fps: u32,
    over_50ms: u32,
    over_100ms: u32,
}

impl Default for FrameStats {
    fn default() -> Self {
        Self {
            window_start: Instant::now(),
            window_index: 0,
            frames: 0,
            total_secs: 0.0,
            max_secs: 0.0,
            over_60fps: 0,
            over_30fps: 0,
            over_50ms: 0,
            over_100ms: 0,
        }
    }
}

impl FrameStats {
    /// Record one frame's delta. Cheap: a few float ops + counter
    /// bumps, no allocation.
    pub fn record(&mut self, delta_secs: f32) {
        self.frames += 1;
        self.total_secs += delta_secs as f64;
        if delta_secs > self.max_secs {
            self.max_secs = delta_secs;
        }
        if delta_secs > BUCKET_60FPS {
            self.over_60fps += 1;
        }
        if delta_secs > BUCKET_30FPS {
            self.over_30fps += 1;
        }
        if delta_secs > BUCKET_50MS {
            self.over_50ms += 1;
        }
        if delta_secs > BUCKET_100MS {
            self.over_100ms += 1;
        }
    }

    /// Whether the current window is due (elapsed >=
    /// [`PERF_WINDOW_SECS`]).
    pub fn window_due(&self, now: Instant) -> bool {
        now.duration_since(self.window_start).as_secs_f64() >= PERF_WINDOW_SECS
    }

    /// Format the window's summary line and reset for the next
    /// window. Returns `None` when no frames were recorded (an
    /// empty window logs nothing — e.g. the app sat in Loading
    /// for the whole window).
    pub fn flush(&mut self, now: Instant) -> Option<String> {
        if self.frames == 0 {
            self.window_start = now;
            return None;
        }
        let avg_ms = self.total_secs / self.frames as f64 * 1000.0;
        let fps = self.frames as f64 / self.total_secs;
        let line = format!(
            "perf: window {}: {} frames, avg {:.1}ms ({:.1} fps), max {:.1}ms, >16.7ms: {}, >33.3ms: {}, >50ms: {}, >100ms: {}",
            self.window_index,
            self.frames,
            avg_ms,
            fps,
            self.max_secs * 1000.0,
            self.over_60fps,
            self.over_30fps,
            self.over_50ms,
            self.over_100ms,
        );
        self.window_index += 1;
        self.frames = 0;
        self.total_secs = 0.0;
        self.max_secs = 0.0;
        self.over_60fps = 0;
        self.over_30fps = 0;
        self.over_50ms = 0;
        self.over_100ms = 0;
        self.window_start = now;
        Some(line)
    }
}

/// Per-frame telemetry: record this frame's delta, and log the
/// window summary when due. Registered InGame-only from
/// `RuffleHudPlugin` — the plugin is added on both app paths, so
/// the logger ships on the phone and runs in desktop screenshot
/// runs alike. (It lives in the Ruffle plugin for registration
/// only; the stats are the whole frame, not the HUD.)
pub fn log_frame_time(time: Res<Time>, mut stats: ResMut<FrameStats>) {
    stats.record(time.delta_secs());
    let now = Instant::now();
    if stats.window_due(now) {
        if let Some(line) = stats.flush(now) {
            crate::startup_log::log(&line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flush_reports_avg_fps_max_and_buckets() {
        let mut stats = FrameStats::default();
        // 100 frames at 16.7ms + 10 frames at 50ms + 1 at 120ms.
        for _ in 0..100 {
            stats.record(1.0 / 60.0);
        }
        for _ in 0..10 {
            stats.record(0.050);
        }
        stats.record(0.120);
        let now = Instant::now() + Duration::from_secs(6);
        assert!(stats.window_due(now));
        let line = stats.flush(now).expect("non-empty window flushes");
        // 111 frames, total 2.287s -> avg 20.6ms, 48.5 fps.
        assert!(line.contains("window 0: 111 frames"), "line was: {line}");
        assert!(line.contains("avg 20.6ms"), "line was: {line}");
        assert!(line.contains("48.5 fps"), "line was: {line}");
        assert!(line.contains("max 120.0ms"), "line was: {line}");
        // >16.7ms: the 10x50ms + 1x120ms = 11 (the 1/60s frames
        // are exactly at the boundary — `>` excludes them).
        assert!(line.contains(">16.7ms: 11"), "line was: {line}");
        assert!(line.contains(">33.3ms: 11"), "line was: {line}");
        assert!(line.contains(">50ms: 1"), "line was: {line}");
        assert!(line.contains(">100ms: 1"), "line was: {line}");
        // The next window starts clean at index 1.
        stats.record(0.016);
        let line2 = stats
            .flush(now + Duration::from_secs(6))
            .expect("second window flushes");
        assert!(line2.contains("window 1: 1 frames"), "line was: {line2}");
    }

    #[test]
    fn flush_on_empty_window_logs_nothing() {
        let mut stats = FrameStats::default();
        let now = Instant::now() + Duration::from_secs(6);
        assert!(stats.window_due(now));
        assert!(stats.flush(now).is_none());
        // The window still advances (no stuck start time).
        assert!(!stats.window_due(now + Duration::from_secs(1)));
    }

    #[test]
    fn window_not_due_before_five_seconds() {
        let stats = FrameStats::default();
        assert!(!stats.window_due(Instant::now()));
        assert!(!stats.window_due(Instant::now() + Duration::from_secs(4)));
    }
}
