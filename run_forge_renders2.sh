#!/bin/bash
# Forge proof renders, part 2: grindstone camera surveys (the
# north close-up in part 1 was blocked by the placeholder boxes
# and the fire blob behind the wheel) + forge-mouth shot +
# wheel/blade regression pairs (new filenames — the original
# milestone proofs are not overwritten).
# Waits for a clear box before each render. Every render log
# records its full args (incl. --camera) on line 1.
cd ~/workspace/dovahengine
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app-forge
# ^ hardlink to the 06:53 build (same inode, zero extra disk):
# Cortana is building on the same tree — if she rebuilds the
# binary mid-batch, cargo's rename leaves this inode untouched,
# so every render in this batch uses MY build, not a mix.

wait_clear() {
  local name="$1" tries=0
  # pgrep -x matches the PROCESS NAME only (pgrep -f false-
  # positives on stale shells whose command text mentions
  # cargo/rustc — part 1 hung on that until fixed).
  # Also waits for OTHER RENDERS: Cortana runs perf renders
  # on the same box, and two renders concurrently OOM-kill
  # each other (grind-NE died exit 137 that way). Any
  # dovah-app / dovah-app-forge process is someone else's —
  # this script's own renders run sequentially, so none of
  # ours is alive at check time.
  while pgrep -x "cargo" > /dev/null || pgrep -x "rustc" > /dev/null || pgrep -x "rust-lld" > /dev/null || pgrep -x "cc" > /dev/null || pgrep -x "dovah-app" > /dev/null || pgrep -x "dovah-app-forge" > /dev/null; do
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

# Grindstone B redo (part 1's run died in the VM restart before
# writing the PNG): 3/4 from the street NW, wheel at 0 deg.
GRIND_B="289.0,1.5,640.0:292.5,-0.7,643.9"
# Grindstone NE: from the north-east, elevated — the fire sits
# ~31 deg off-axis instead of dead behind the wheel, and the
# placeholder boxes are behind the camera. Wheel pivot
# (292.47,-0.65,643.94).
GRIND_NE="294.5,0.5,641.5:292.47,-0.65,643.94"
# Mill grindstone (ref 0x0006f947, same NIF/base as the forge
# one, no fire behind it): -axle camera. Wheel pivot
# (291.03,-0.99,612.11), axle (0.374,0,-0.927).
MILL_GRIND="290.1,-0.2,614.4:291.03,-0.99,612.11"
# Forge mouth: from inside the shed looking at the fire/coals
# (fire light at 293.7,0.0,647.9).
MOUTH="291.0,0.0,644.5:293.5,-0.5,648.0"
# Regressions: wheel camC (mill milestone), blade camB (sawmill).
WHEEL_CAM="290,2.5,635:281.7,1.1,626.3"
BLADE_CAM="280.0,3.0,622.3:281.7,1.1,626.3"

# grind-B DONE in the first run of this script (exit 0,
# screenshots/forge-grind-B.png, water cells 6) — skipped on
# restart (the script was killed after grind-NE OOMed on a
# concurrent perf render).
# run_render grind-B --exterior 5 -11 --camera "$GRIND_B" --screenshot screenshots/forge-grind-B.png --no-normal-maps --frames 10 --sawmill-time 2.0
run_render grind-NE --exterior 5 -11 --camera "$GRIND_NE" --screenshot screenshots/forge-grind-NE.png --no-normal-maps --frames 10 --sawmill-time 2.0
run_render mill-grind --exterior 5 -11 --camera "$MILL_GRIND" --screenshot screenshots/forge-mill-grind.png --no-normal-maps --frames 10 --sawmill-time 2.0
run_render mouth --exterior 5 -11 --camera "$MOUTH" --screenshot screenshots/forge-mouth.png --no-normal-maps --frames 10 --sawmill-time 2.0
run_render wheel-f10 --exterior 5 -11 --camera "$WHEEL_CAM" --screenshot screenshots/forge-wheel-f10.png --no-normal-maps --frames 10
run_render wheel-f60 --exterior 5 -11 --camera "$WHEEL_CAM" --screenshot screenshots/forge-wheel-f60.png --no-normal-maps --frames 60
run_render blade-still --exterior 5 -11 --camera "$BLADE_CAM" --screenshot screenshots/forge-blade-still.png --no-normal-maps --frames 10 --sawmill-time 10
run_render blade-spinning --exterior 5 -11 --camera "$BLADE_CAM" --screenshot screenshots/forge-blade-spinning.png --no-normal-maps --frames 10 --sawmill-time 33
echo "ALL FORGE RENDERS PART 2 DONE $(date)"
md5sum screenshots/forge-grind-B.png screenshots/forge-grind-NE.png screenshots/forge-mill-grind.png screenshots/forge-mouth.png screenshots/forge-wheel-f10.png screenshots/forge-wheel-f60.png screenshots/forge-blade-still.png screenshots/forge-blade-spinning.png 2>/dev/null
echo "--- water cells per log (guard proof: 6 every time) ---"
grep -H "water cells" forge_render_grind-B.log forge_render_grind-NE.log forge_render_mill-grind.log forge_render_mouth.log forge_render_wheel-f10.log forge_render_wheel-f60.log forge_render_blade-still.log forge_render_blade-spinning.log 2>/dev/null
