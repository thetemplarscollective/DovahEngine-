#!/bin/bash
# Inn-lighting AFTER renders (ambient 200) + exterior crash-log proof.
# Uses the NEW binary (target/debug/dovah-app, built after the
# ambient + phase-log edits). Waits for a clear box via pgrep -x.
cd ~/workspace/dovahengine
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app-light

wait_clear() {
  while pgrep -x cargo >/dev/null || pgrep -x rustc >/dev/null || pgrep -x rust-lld >/dev/null || pgrep -x cc >/dev/null || pgrep -x dovah-app >/dev/null || pgrep -x dovah-app-forge >/dev/null; do
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
  echo "BIN: $BIN (ambient 200 + phase-boundary logs)" >> "light_${name}.log"
  echo "START: $(date)" >> "light_${name}.log"
  $BIN "$@" >> "light_${name}.log" 2>&1
  echo "EXIT: $? $(date)" >> "light_${name}.log"
  echo "=== $name: exit $? $(date) ==="
}

# 1. Spawn view (default viewpoint) — after
run_one after-spawn --no-normal-maps --frames 10 \
  --screenshot screenshots/light-after-spawn.png
# 2. Counter view (Orgnar's face, counter top) — after
#    camera eye -14.5,1.6,-0.3 look-at -18.75,1.6,-0.26 (same as before)
run_one after-counter --no-normal-maps --frames 10 \
  --camera -14.5,1.6,-0.3:-18.75,1.6,-0.26 \
  --screenshot screenshots/light-after-counter.png
# 3. Exterior (crash-log proof): full 9-cell terrain + water + sky +
#    finish, default viewpoint at cell (5,-11). The LOG is the proof
#    here (phase-boundary lines in sequence + water cells 6).
run_one after-ext --exterior 5 -11 --no-normal-maps --frames 10 \
  --screenshot screenshots/light-after-ext.png
echo "=== ALL AFTER RENDERS DONE $(date) ==="
