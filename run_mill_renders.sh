#!/bin/bash
# Mill wheel proof renders: inn regression + mill f10/f60 pair.
# Waits for a clear box (no cargo/rustc/lld) before each render.
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
  "$BIN" "$@" > "mill_render_${name}.log" 2>&1
  echo "=== $name: exit $? $(date +%H:%M:%S) ==="
}

# 1. Inn regression (default viewpoint, f10) — must match v047b-inn-spawn md5.
run_render inn --screenshot screenshots/mill-inn-spawn.png --no-normal-maps --frames 10
# 2. Mill wheel f10 — camera on the river side, looking at the
# wheel close-up. NOTE (sawmill milestone): the original camera
# here (298,3,645:285,-1,632) sits INSIDE the mill shed's thatch
# roof — a re-render with it shows only thatch (see
# screenshots/sawmill-mill-f10.png). The published mill-ext
# renders came from a later "f10b" run whose camera coords were
# never recorded; the camC camera below reproduces a clear
# wheel view and proves the turn (see
# screenshots/sawmill-wheel-diff-overlay.png).
run_render mill-f10 --exterior 5 -11 --camera 290,2.5,635:281.7,1.1,626.3 --screenshot screenshots/mill-ext-f10.png --no-normal-maps --frames 10
# 3. Mill wheel f60 — same camera, 50 more frames.
run_render mill-f60 --exterior 5 -11 --camera 290,2.5,635:281.7,1.1,626.3 --screenshot screenshots/mill-ext-f60.png --no-normal-maps --frames 60
echo "ALL MILL RENDERS DONE $(date)"
md5sum screenshots/mill-inn-spawn.png screenshots/mill-ext-f10.png screenshots/mill-ext-f60.png 2>/dev/null
