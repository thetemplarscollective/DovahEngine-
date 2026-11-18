#!/bin/bash
# Inn-lighting BEFORE renders (ambient 40, 06:53 forge binary).
# Uses the hardlinked dovah-app-forge binary (exact binary that made
# forge-inn-spawn.png) so the before/after pair differs ONLY by the
# lighting fix, not by Cortana's concurrent perf-instrumentation build.
# Waits for a clear box (builds AND renders) via pgrep -x.
cd ~/workspace/dovahengine
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app-forge

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
  echo "BIN: $BIN (06:53 forge build, ambient 40)" >> "light_${name}.log"
  echo "START: $(date)" >> "light_${name}.log"
  $BIN "$@" >> "light_${name}.log" 2>&1
  echo "EXIT: $? $(date)" >> "light_${name}.log"
  echo "=== $name: exit $? $(date) ==="
}

# 1. Spawn view (default viewpoint) — before
run_one before-spawn --no-normal-maps --frames 10 \
  --screenshot screenshots/light-before-spawn.png
# 2. Counter view (Orgnar's face, counter top) — before
#    camera eye -14.5,1.6,-0.3 look-at -18.75,1.6,-0.26 (v047b counter camera)
run_one before-counter --no-normal-maps --frames 10 \
  --camera -14.5,1.6,-0.3:-18.75,1.6,-0.26 \
  --screenshot screenshots/light-before-counter.png
echo "=== ALL BEFORE RENDERS DONE $(date) ==="
