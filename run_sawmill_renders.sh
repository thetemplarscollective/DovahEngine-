#!/bin/bash
# Sawmill proof renders: inn regression + blade still/spinning pair.
# Waits for a clear box (no cargo/rustc/lld) before each render.
# The pair differs by --sawmill-time, NOT by frame count: both are
# --frames 10, so the wheel and water are identical in both and
# the pixel-diff is blade-only. --sawmill-time 10 = blade at rest
# (0 deg, still period); --sawmill-time 33 = mid first burst
# (~540 deg, 1.5 turns in). Camera is 4 m off the blade along its
# axle (probe Part F: pivot 281.7,1.1,626.3).
cd ~/workspace/dovahengine
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app

wait_clear() {
  local name="$1" tries=0
  while pgrep -f "cargo (build|test)|rustc|rust-lld|cargo-apk" > /dev/null; do
    if [ $tries -eq 0 ]; then echo "=== $name: box busy, waiting ==="; fi
    sleep 60; tries=$((tries+1))
    if [ $tries -gt 60 ]; then echo "=== $name: BOX NEVER CLEARED, aborting ==="; exit 1; fi
  done
}

run_render() {
  local name="$1"; shift
  wait_clear "$name"
  echo "=== $name: starting $(date +%H:%M:%S) ==="
  "$BIN" "$@" > "sawmill_render_${name}.log" 2>&1
  echo "=== $name: exit $? $(date +%H:%M:%S) ==="
}

CAM="280.0,3.0,622.3:281.7,1.1,626.3"
# (camB from the camera survey: the first close-up
# 278.9,2.6,623.4 put a centre post across the blade, and the
# axle's other side 284.5,2.6,629.1 sits against a post. camB
# threads between the posts and shows the blade's top arc.)

# 1. Inn regression (default viewpoint, f10) — must match mill-inn-spawn md5.
run_render inn --screenshot screenshots/sawmill-inn-spawn.png --no-normal-maps --frames 10
# 2. Blade at rest (still period, 0 deg).
run_render blade-still --exterior 5 -11 --camera "$CAM" --screenshot screenshots/sawmill-ext-f10.png --no-normal-maps --frames 10 --sawmill-time 10
# 3. Blade mid-spin (first burst, ~540 deg).
run_render blade-spinning --exterior 5 -11 --camera "$CAM" --screenshot screenshots/sawmill-ext-f60.png --no-normal-maps --frames 10 --sawmill-time 33
echo "ALL SAWMILL RENDERS DONE $(date)"
md5sum screenshots/sawmill-inn-spawn.png screenshots/sawmill-ext-f10.png screenshots/sawmill-ext-f60.png 2>/dev/null
