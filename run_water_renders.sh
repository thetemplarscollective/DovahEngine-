#!/bin/bash
# Water milestone proof renders, sequential (one lavapipe render
# at a time — two OOM the 6 GB box). Waits for a clear box first
# (no other cargo build or render running).
cd ~/workspace/dovahengine || exit 1
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app

wait_clear() {
    for i in $(seq 1 60); do
        if ! pgrep -f "cargo (build|test|run|clippy)" >/dev/null \
           && ! pgrep -f "target/debug/dovah-app" >/dev/null; then
            return 0
        fi
        sleep 10
    done
    echo "BOX NEVER CLEARED" >&2
    return 1
}

run_render() {
    local name="$1"; shift
    echo "=== $name: waiting for clear box ==="
    wait_clear || return 1
    echo "=== $name: starting $(date +%H:%M:%S) ==="
    "$BIN" "$@" > "water_render_${name}.log" 2>&1
    echo "=== $name: exit $? $(date +%H:%M:%S) ==="
}

# 1. Inn regression — must stay byte-identical to v047b-inn-spawn.
run_render inn --screenshot screenshots/water-inn-spawn.png --no-normal-maps --frames 10

# 2. Exterior spawn — compare vs v047b-ext-spawn (water off-camera).
run_render ext-spawn --exterior 5 -11 --screenshot screenshots/water-ext-spawn.png --no-normal-maps --frames 10

# 3. Bridge viewpoint, frames 10 — the water proof. Camera on the
# bridge deck (walk-sim bridge start), eye height, looking NE
# along the river (the flow direction) and down at the surface.
run_render ext-bridge --exterior 5 -11 --camera 346.9,0.4,564.5:369,-3.5,552.5 --screenshot screenshots/water-ext-bridge.png --no-normal-maps --frames 10

# 4. Same camera, frames 60 — the flow proof (diff vs f10 in the
# water region: 50 more frames at 30 fps = 1.67 s of scroll).
run_render ext-bridge-f60 --exterior 5 -11 --camera 346.9,0.4,564.5:369,-3.5,552.5 --screenshot screenshots/water-ext-bridge-f60.png --no-normal-maps --frames 60

echo "ALL WATER RENDERS DONE $(date)"
