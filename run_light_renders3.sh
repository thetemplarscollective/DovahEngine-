#!/bin/bash
# Inn-lighting AFTER-100 renders (ambient 100) + exterior re-run
# (proves the "terrain done" boundary log fires exactly once
# with the terrain_done_logged guard, and water cells still 6).
# Uses the NEW binary (target/debug/dovah-app, built after the
# ambient 100 + terrain-log-guard edits). Waits for a clear box.
cd ~/workspace/dovahengine
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app-light100

wait_clear() {
  while pgrep -x cargo >/dev/null || pgrep -x rustc >/dev/null || pgrep -x rust-lld >/dev/null || pgrep -x cc >/dev/null || pgrep -x dovah-app >/dev/null || pgrep -x dovah-app-forge >/dev/null || pgrep -x dovah-app-light >/dev/null || pgrep -x dovah-app-light100 >/dev/null; do
    echo "=== $1: box busy, waiting ==="
    sleep 60
  done
}

run_one() {
  local name="$1"; shift
  wait_clear "$name"
  echo "=== $name: starting $(date) ==="
  echo "ARGS: $*" > "light_${name}.log"
  echo "CAMERA: see ARGS" >> "light_${name}.log"
  echo "BIN: $BIN (ambient 100 + terrain-log guard)" >> "light_${name}.log"
  echo "START: $(date)" >> "light_${name}.log"
  $BIN "$@" >> "light_${name}.log" 2>&1
  echo "EXIT: $? $(date)" >> "light_${name}.log"
  echo "=== $name: exit $? $(date) ==="
}

# 1. Counter view (Orgnar's face) — after-100
run_one after100-counter --no-normal-maps --frames 10 \
  --camera -14.5,1.6,-0.3:-18.75,1.6,-0.26 \
  --screenshot screenshots/light-after100-counter.png
# 2. Spawn view (default viewpoint) — after-100
run_one after100-spawn --no-normal-maps --frames 10 \
  --screenshot screenshots/light-after100-spawn.png
# 3. Exterior re-run (terrain-log-once + water-6 proof)
run_one after100-ext --exterior 5 -11 --no-normal-maps --frames 10 \
  --screenshot screenshots/light-after100-ext.png
# 4. Counter CLOSE-UP (dark-tint measurement): matches Dylan's
#    annotated phone frame — close to the counter, looking down
#    at the bottle/mug/plate cluster. Eye (-16.2,1.9,0.8),
#    look-at (-18.3,1.1,-0.2). The shadows-off isolation test
#    re-renders this SAME camera for a clean A/B pair.
run_one after100-closeup --no-normal-maps --frames 10 \
  --camera -16.2,1.9,0.8:-18.3,1.1,-0.2 \
  --screenshot screenshots/light-after100-closeup.png
echo "=== ALL AFTER-100 RENDERS DONE $(date) ==="
