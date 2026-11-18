#!/bin/bash
# v0.4.6 phone-feedback renders, round 2 (rev 2) — re-runs the 4
# renders OOM-killed (exit 137) in round 1. Render 1 (door-open)
# succeeded and is NOT re-run. Priority order: guard (outfit
# proof) first, then waterfall (yellow arrow), hud, inn.
#
# Rev 2 (2026-11-16 ~05:00 UTC, after VM restart #4): the rev-1
# RAM guard (4500 MB available) NEVER fired in ~3 hours — the
# box stays loaded by other agents' jobs (node tests, coh-client
# renders, Ruffle M1). New strategy: guard at 3500 MB (a render
# peaks ~2863 MB RSS, so 3500 leaves ~600 MB headroom) + up to
# 3 attempts per render, retrying after 5 min on an OOM kill
# (exit 137). A non-137 failure aborts that render immediately.
# Waits for lavapipe first (restarts wipe mesa-vulkan-drivers;
# the runtime reconcile restores it in ~5 min).
cd ~/workspace/dovahengine || exit 1
rm -f renders2_done.txt
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app

for i in $(seq 1 90); do
    [ -f /usr/share/vulkan/icd.d/lvp_icd.json ] && break
    sleep 10
done
if [ ! -f /usr/share/vulkan/icd.d/lvp_icd.json ]; then
    echo "LAVAPIPE STILL MISSING after 15 min — renders cannot run" > renders2_done.txt
    exit 1
fi
echo "lavapipe present, starting round-2 renders $(date)"

wait_for_ram() {
    local waited=0
    while true; do
        local avail
        avail=$(free -m | awk '/^Mem:/ {print $7}')
        if [ "$avail" -ge 3500 ]; then
            echo "ram ok: ${avail} MB available, starting $1 $(date)" >> "render_$1_2.log"
            return 0
        fi
        if [ $((waited % 10)) -eq 0 ]; then
            echo "waiting for ram: ${avail} MB available, need 3500 (for $1) $(date)" >> "render_$1_2.log"
        fi
        waited=$((waited + 1))
        sleep 60
    done
}

run_render() {
    local name="$1"; shift
    local attempt=1
    : > "render_${name}2.log"
    while [ $attempt -le 3 ]; do
        wait_for_ram "$name"
        echo "=== attempt $attempt $(date) ===" >> "render_${name}2.log"
        $BIN "$@" >> "render_${name}2.log" 2>&1
        local code=$?
        echo "$name attempt $attempt exit $code $(date)" >> "render_${name}2.log"
        if [ $code -eq 0 ]; then
            return 0
        fi
        if [ $code -ne 137 ]; then
            return $code
        fi
        attempt=$((attempt + 1))
        [ $attempt -le 3 ] && sleep 300
    done
    return 137
}

run_render guard --exterior 5 -11 --camera 264,-0.3,668:264,-1.0,673 --screenshot screenshots/v046-guard-clothed.png --no-normal-maps --frames 10
run_render waterfall --exterior 5 -11 --camera 332,2,635:300,5,600 --screenshot screenshots/v046-waterfall-north.png --no-normal-maps --frames 10
run_render hud --screenshot-ui --screenshot screenshots/v046-hud-ingame.png --no-normal-maps --frames 10
run_render inn --screenshot screenshots/v046-inn-regression.png --no-normal-maps --frames 10

echo "ALL ROUND-2 RENDERS DONE $(date)" > renders2_done.txt
