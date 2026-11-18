#!/bin/bash
# Drop-diagnosis singles: barnacle alone + mask alone, inn lowered
# camera — the pile hid the small items at the sword's hilt.
# Same RAM guard (3500 MB) + 3 attempts on OOM kill.
cd ~/workspace/dovahengine || exit 1
rm -f drops2_done.txt
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app
CAM="--camera -14.30,1.95,-4.55:-13.50,0.25,-4.55"
wait_for_ram() {
    local waited=0
    while true; do
        local avail
        avail=$(free -m | awk '/^Mem:/ {print $7}')
        if [ "$avail" -ge 3500 ]; then return 0; fi
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
        $BIN "$@" >> "render_${name}.log" 2>&1
        local code=$?
        echo "$name attempt $attempt exit $code $(date)" >> "render_${name}.log"
        if [ $code -eq 0 ]; then return 0; fi
        if [ $code -ne 137 ]; then return $code; fi
        attempt=$((attempt + 1))
        [ $attempt -le 3 ] && sleep 300
    done
    return 137
}
run_render drop_barnacle $CAM --drop-test 7edf5 --screenshot screenshots/drop-barnacle.png --no-normal-maps --frames 10
run_render drop_mask $CAM --drop-test 61cc1 --screenshot screenshots/drop-mask.png --no-normal-maps --frames 10
echo "ALL DROP SINGLES DONE $(date)" > drops2_done.txt
