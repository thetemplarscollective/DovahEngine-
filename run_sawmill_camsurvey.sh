#!/bin/bash
# Sawmill camera survey: 4 candidate viewpoints, all with the
# blade mid-spin (--sawmill-time 33), to find a clear view of
# the blade disc. The first close-up (278.9,2.6,623.4) was
# occluded by the deck + a centre post.
cd ~/workspace/dovahengine
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app
PIVOT="281.7,1.1,626.3"

run_render() {
  local name="$1" cam="$2"
  echo "=== $name: starting $(date +%H:%M:%S) ==="
  "$BIN" --exterior 5 -11 --camera "$cam:$PIVOT" --screenshot "screenshots/sawmill-$name.png" --no-normal-maps --frames 10 --sawmill-time 33 > "sawmill_render_$name.log" 2>&1
  echo "=== $name: exit $? $(date +%H:%M:%S) ==="
}

# A: other side of the axle (probe's suggestion).
run_render camA "284.5,2.6,629.1"
# B: lateral shift (thread between posts) + raised.
run_render camB "280.0,3.0,622.3"
# C: river side, medium distance.
run_render camC "290,2.5,635"
# D: low + lateral (under the deck overhang).
run_render camD "280.0,0.9,622.3"
echo "CAMERA SURVEY DONE $(date)"
ls -la screenshots/sawmill-cam*.png 2>/dev/null
