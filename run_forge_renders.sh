#!/bin/bash
# Forge proof renders, part 1: inn regression + forge overview +
# grindstone camera survey (two candidates, both at
# --sawmill-time 2.0 = wheel at 0 deg). Part 2 runs the winning
# camera at the second time + the wheel/blade regression pairs.
# Waits for a clear box (no cargo/rustc/lld) before each render.
# Every render log records its full args (incl. --camera) on
# line 1 — the mill script's unrecorded camera bit us.
cd ~/workspace/dovahengine
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app

wait_clear() {
  local name="$1" tries=0
  # pgrep -x matches the PROCESS NAME only: pgrep -f matches
  # cmdlines, and stale shells whose command text mentions
  # cargo/rustc (e.g. an earlier `sleep 240; ps ... cargo`)
  # false-positive forever and the script waits 60 min then
  # aborts. cargo/rustc/rust-lld/cc are the real build procs.
  while pgrep -x "cargo" > /dev/null || pgrep -x "rustc" > /dev/null || pgrep -x "rust-lld" > /dev/null || pgrep -x "cc" > /dev/null; do
    if [ $tries -eq 0 ]; then echo "=== $name: box busy, waiting ==="; fi
    sleep 60; tries=$((tries+1))
    if [ $tries -gt 60 ]; then echo "=== $name: BOX NEVER CLEARED, aborting ==="; exit 1; fi
  done
}

run_render() {
  local name="$1"; shift
  wait_clear "$name"
  echo "=== $name: starting $(date +%H:%M:%S) ==="
  echo "ARGS: $*" > "forge_render_${name}.log"
  echo "START: $(date)" >> "forge_render_${name}.log"
  "$BIN" "$@" >> "forge_render_${name}.log" 2>&1
  echo "=== $name: exit $? $(date +%H:%M:%S) ==="
}

# Forge overview (probe Part E): from the street north-west of
# the forge, looking at the forge mouth (marker 292.3,-1.2,646.1).
OVERVIEW_CAM="286.0,2.0,640.0:292.3,-0.5,646.1"
# Grindstone A: -axle close-up (probe Part E). Wheel pivot
# (292.47,-0.65,643.94), axle (0.020,0,1.000) — the +axle side
# sits inside the forge, so shoot from the north (-axle).
GRIND_A="292.4,0.1,641.4:292.5,-0.7,643.9"
# Grindstone B: wider 3/4 from the street, wheel face at an
# angle with the forge behind it.
GRIND_B="289.0,1.5,640.0:292.5,-0.7,643.9"

# 1. Inn regression (default viewpoint, f10) — pixel-diff vs
# sawmill-inn-spawn.png (md5 44dfd4f77d1cfea4fb301c6fee4d0826);
# 8 dither px / <=3 LSB is the known floor.
run_render inn --screenshot screenshots/forge-inn-spawn.png --no-normal-maps --frames 10
# 2. Forge overview.
run_render overview --exterior 5 -11 --camera "$OVERVIEW_CAM" --screenshot screenshots/forge-overview.png --no-normal-maps --frames 10
# 3. Grindstone camera A, wheel at 0 deg (--sawmill-time 2.0 =
# start of the Wheel track; the blade is in its 0-15.37 s hold
# at both times, so it stays put).
run_render grind-A --exterior 5 -11 --camera "$GRIND_A" --screenshot screenshots/forge-grind-A.png --no-normal-maps --frames 10 --sawmill-time 2.0
# 4. Grindstone camera B, wheel at 0 deg.
run_render grind-B --exterior 5 -11 --camera "$GRIND_B" --screenshot screenshots/forge-grind-B.png --no-normal-maps --frames 10 --sawmill-time 2.0
echo "ALL FORGE RENDERS PART 1 DONE $(date)"
md5sum screenshots/forge-inn-spawn.png screenshots/forge-overview.png screenshots/forge-grind-A.png screenshots/forge-grind-B.png 2>/dev/null
echo "--- water cells per log (guard proof: 6 every time) ---"
grep -H "water cells" forge_render_*.log 2>/dev/null
