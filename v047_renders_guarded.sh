#!/bin/bash
# v0.4.7 phone-feedback round: guarded proof renders.
# Runs each render only when NO other dovah-app render and NO
# cargo build/test/link is running (the OOM rule). Polls every
# 20s so it slips into the gaps between Cortana's M4 renders
# instead of racing her. Never runs two renders concurrently.
# Logs to v047_renders_guarded.log. Each render writes its PNG
# as it completes, so partial progress survives interruption.
cd ~/workspace/dovahengine
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_MANIFEST_DIR="$PWD/crates/dovah-app"
BIN=./target/debug/dovah-app

wait_clear() {
    while true; do
        if pgrep -f "dovah-app --" >/dev/null; then
            sleep 20
            continue
        fi
        if pgrep -x cargo >/dev/null || pgrep -x rust-lld >/dev/null || pgrep -x rustc >/dev/null; then
            sleep 20
            continue
        fi
        break
    done
}

run_one() {
    local name="$1"
    shift
    echo "=== $name: waiting for clear box ==="
    wait_clear
    echo "=== $name: starting $(date +%H:%M:%S) ==="
    "$BIN" "$@"
    echo "=== $name: exit $? $(date +%H:%M:%S) ==="
}

# 1. Inn spawn view — ambient before/after vs screenshots/load-test-inn.png
run_one "inn-spawn" --no-normal-maps --frames 10 --screenshot screenshots/v047-inn-spawn.png
# 2. Inn counter view (Orgnar's face, white streaks, counter top) vs phone shot 06
run_one "inn-counter" --no-normal-maps --frames 10 --camera -14.5,1.6,-0.3:-18.75,1.6,-0.26 --screenshot screenshots/v047-inn-counter.png
# 3. Inn door view vs phone shot 05
run_one "inn-door" --no-normal-maps --frames 10 --camera -3.65,1.6,0:-3.65,1.4,6.04 --screenshot screenshots/v047-inn-door.png
# 4. Exterior spawn view (markers + white) vs screenshots/load-test-exterior.png
run_one "ext-spawn" --exterior 5 -11 --no-normal-maps --frames 10 --screenshot screenshots/v047-ext-spawn.png
# 5. Exterior bridge view vs phone shot 01
run_one "ext-bridge" --exterior 5 -11 --no-normal-maps --frames 10 --camera 347,2,564:347,0,540 --screenshot screenshots/v047-ext-bridge.png
echo "ALL V047 RENDERS DONE $(date +%H:%M:%S)"
