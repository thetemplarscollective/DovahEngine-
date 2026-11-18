#!/bin/bash
# Archery re-shoot 2: a slow 5 m/s arrow, which the counter
# cannot swallow — mid-flight at ~1.75 m in a --frames 1 run
# (~140 px against the floor), and stuck in the open floor at
# ~3.1 m in a --frames 10 run. Same flight camera as the first
# runs, so the path is identical (teleport (-12.80, 5.08) ->
# Orgnar). The --camera look-at is discarded by the seed (the
# seed faces the player at Orgnar's chest); only the eye's xz
# matters, via the teleport direction.
# Control: screenshots/archery-flight-control.png (same camera,
# --frames 1, 0 shots) is reused for both diffs — the inn
# scene is static, so the frame count does not change it.
set -u
cd ~/workspace/dovahengine
BIN=./target/debug/dovah-app-archery
CAM="-14.0,2.4,4.0:-16.0,1.0,-0.3"

# 1. MID-FLIGHT: 1 shot at 5 m/s, 1 frame (~0.35 s of flight
# -> arrow at ~1.75 m, y ~1.0 m, ~18 deg below the view axis).
{
    echo "ARGS: --no-normal-maps --frames 1 --archery-test 1 --archery-speed 5 --camera $CAM --screenshot screenshots/archery-midflight.png"
    echo "CAMERA: eye (-14.0,2.4,4.0) — sets the teleport ((-12.80,1.70,5.08)); the seed faces the player at Orgnar's chest; arrow expected at ~(-14.1,1.0,3.9), lower-center frame"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 1 --archery-test 1 --archery-speed 5 \
        --camera "$CAM" \
        --screenshot screenshots/archery-midflight.png 2>&1
    echo "EXIT: $? $(date -u)"
} > archery_midflight.log 2>&1

# 2. FLOOR-STUCK: 1 shot at 5 m/s, 10 frames (the arrow lands
# at ~0.62 s, ~3.1 m out, and sticks in the open floor).
{
    echo "ARGS: --no-normal-maps --frames 10 --archery-test 1 --archery-speed 5 --camera $CAM --screenshot screenshots/archery-floorstuck.png"
    echo "CAMERA: same as archery-midflight.png; arrow expected stuck in the floor at ~(-15.1,0,3.0), lower-center frame"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 10 --archery-test 1 --archery-speed 5 \
        --camera "$CAM" \
        --screenshot screenshots/archery-floorstuck.png 2>&1
    echo "EXIT: $? $(date -u)"
} > archery_floorstuck.log 2>&1

echo "reshoot2 done"
grep -h "archery-test:\|Fired\|Shot\|stuck" archery_midflight.log archery_floorstuck.log
