#!/bin/bash
# Drop-diagnosis renders: drop an iron sword (0x647ac), a Nordic
# Barnacle (0x7edf5) and a dragon priest mask (0x61cc1, the ARMO
# ground-model stand-in) via --drop-test, and LOOK at where they
# land. Two views per scene, same drop spot: the level view (the
# phone experience — the drop should be below the frame) and a
# lowered --camera at the same eye, looking down at the exact
# drop point from the probe (are the items there but sunk?).
# RAM guard at 3500 MB + up to 3 attempts per render on an OOM
# kill (exit 137), same as the round-2 rev-2 script.
cd ~/workspace/dovahengine || exit 1
rm -f drops_done.txt
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app
DROP="--drop-test 647ac,7edf5,61cc1"

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
    : > "render_${name}.log"
    while [ $attempt -le 3 ]; do
        wait_for_ram "$name"
        echo "=== attempt $attempt $(date) ===" >> "render_${name}.log"
        $BIN "$@" >> "render_${name}.log" 2>&1
        local code=$?
        echo "$name attempt $attempt exit $code $(date)" >> "render_${name}.log"
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

# Inn, lowered (the sinking proof — flat floor, controlled).
run_render drop_inn_down --camera -14.30,1.95,-4.55:-13.50,0.25,-4.55 $DROP --screenshot screenshots/drop-inn-down.png --no-normal-maps --frames 10
# Inn, level (the phone experience).
run_render drop_inn_level $DROP --screenshot screenshots/drop-inn-level.png --no-normal-maps --frames 10
# Exterior, lowered (porch drop point).
run_render drop_ext_down --exterior 5 -11 --camera 332.28,1.58,635.37:331.69,-0.12,634.84 $DROP --screenshot screenshots/drop-ext-down.png --no-normal-maps --frames 10
# Exterior, level.
run_render drop_ext_level --exterior 5 -11 $DROP --screenshot screenshots/drop-ext-level.png --no-normal-maps --frames 10

echo "ALL DROP RENDERS DONE $(date)" > drops_done.txt
