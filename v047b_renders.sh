#!/bin/bash
# v0.4.7 round B renders: face-normal flip + placeholder fix proof.
# Runs each render only when the box is clear (no cargo/rust-lld/
# dovah-app running) so it never OOMs against a build.
cd ~/workspace/dovahengine || exit 1
export CARGO_MANIFEST_DIR=crates/dovah-app
BIN=./target/debug/dovah-app

wait_clear() {
  while pgrep -f "cargo build|cargo test|rust-lld|rustc|dovah-app --" >/dev/null 2>&1; do
    sleep 20
  done
}

run() {
  local name="$1"; shift
  echo "=== $name: waiting for clear box ==="
  wait_clear
  echo "=== $name: starting $(date +%H:%M:%S) ==="
  "$BIN" "$@" --no-normal-maps --frames 10 --screenshot "screenshots/$name.png" 2>&1
  echo "=== $name: exit $? $(date +%H:%M:%S) ==="
}

run v047b-inn-counter --camera -14.5,1.6,-0.3:-18.75,1.6,-0.26
run v047b-ext-spawn --exterior 5 -11
run v047b-ext-bridge --exterior 5 -11 --camera 347,2,564:347,0,540
echo "=== ALL DONE $(date +%H:%M:%S) ==="
