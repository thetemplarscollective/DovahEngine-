#!/bin/bash
# Water milestone: re-run the two renders the first script
# skipped (another agent's cargo work held the box).
cd ~/workspace/dovahengine || exit 1
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app

wait_clear() {
    # Up to 45 min: a render (~3 GB RSS) must not run while
    # another agent's cargo build/link holds the 6 GB box.
    for i in $(seq 1 90); do
        if ! pgrep -f "cargo|rustc|rust-lld" >/dev/null \
           && ! pgrep -f "target/debug/dovah-app" >/dev/null; then
            return 0
        fi
        sleep 30
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

run_render ext-spawn --exterior 5 -11 --screenshot screenshots/water-ext-spawn.png --no-normal-maps --frames 10
run_render ext-bridge --exterior 5 -11 --camera 346.9,0.4,564.5:369,-3.5,552.5 --screenshot screenshots/water-ext-bridge.png --no-normal-maps --frames 10

echo "WATER RERUNS DONE $(date)"
