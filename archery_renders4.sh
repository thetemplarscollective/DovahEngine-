#!/bin/bash
# Fix test: re-render the two stuck-arrow shots with the
# GlobalTransform+Visibility fix on the arrow root. If the
# fix works, the stuck arrow is visible in both (it was 0 px
# before) and B0004 is gone from the logs.
set -u
cd ~/workspace/dovahengine
BIN=./target/debug/dovah-app-archery
{
    echo "ARGS: --no-normal-maps --frames 10 --archery-test 1 --archery-speed 5 --camera -14.0,2.4,4.0:-16.0,1.0,-0.3 --screenshot screenshots/archery-floorstuck2.png"
    echo "CAMERA: eye (-14.0,2.4,4.0) — sets the teleport ((-12.80,1.70,5.08)); arrow expected stuck in the floor at ~(-15.1,0,3.0), lower-center frame"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 10 --archery-test 1 --archery-speed 5 \
        --camera -14.0,2.4,4.0:-16.0,1.0,-0.3 \
        --screenshot screenshots/archery-floorstuck2.png 2>&1
    echo "EXIT: $? $(date -u)"
} > archery_floorstuck2.log 2>&1
{
    echo "ARGS: --no-normal-maps --frames 10 --archery-test 1 --archery-speed 15 --camera -15.03,2.4,3.08:-16.6,0.8,1.7 --screenshot screenshots/archery-stuck-closeup2.png"
    echo "CAMERA: eye (-15.03,2.4,3.08) look-at (-16.6,0.8,1.7) — 2 m from the counter stick point"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 10 --archery-test 1 --archery-speed 15 \
        --camera -15.03,2.4,3.08:-16.6,0.8,1.7 \
        --screenshot screenshots/archery-stuck-closeup2.png 2>&1
    echo "EXIT: $? $(date -u)"
} > archery_stuck_closeup2.log 2>&1
echo "fix-test done"
grep -h "Fired\|stuck" archery_floorstuck2.log archery_stuck_closeup2.log
echo "B0004 count:"; grep -c "B0004" archery_floorstuck2.log archery_stuck_closeup2.log
