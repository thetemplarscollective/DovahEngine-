#!/bin/bash
# Melee proof renders: before (standing Orgnar) + after (fallen
# Orgnar) + regression (standard close-up vs
# light-after100-closeup.png). Run AFTER the dovah-app bin is
# built and copied to target/debug/dovah-app-melee.
# Camera coords are recorded in each render log (the melee-test
# seed logs its teleport position; the regression uses the
# light-after100 close-up camera).
set -u
cd ~/workspace/dovahengine
BIN=./target/debug/dovah-app-melee

# 1. BEFORE: teleport in front of Orgnar, 0 attacks, 10 frames.
{
    echo "ARGS: --no-normal-maps --frames 10 --melee-test 0 --screenshot screenshots/melee-before.png"
    echo "CAMERA: melee-test seed teleport (1.2m from Orgnar, facing his chest) — see the melee-test log line below"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 10 --melee-test 0 \
        --screenshot screenshots/melee-before.png 2>&1
    echo "EXIT: $? $(date -u)"
} > melee_before.log 2>&1

# 2. AFTER: 25 queued fist attacks, 100 frames. Fist cooldown is
# 1.0s and Bevy clamps frame deltas to 250ms, so one hit lands
# every 4th frame; Orgnar (75 HP / 4 dmg) dies on hit 19, frame
# ~73. The final frame shows the fallen body.
{
    echo "ARGS: --no-normal-maps --frames 100 --melee-test 25 --screenshot screenshots/melee-after.png"
    echo "CAMERA: melee-test seed teleport (same as melee-before.png) — see the melee-test log line below"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 100 --melee-test 25 \
        --screenshot screenshots/melee-after.png 2>&1
    echo "EXIT: $? $(date -u)"
} > melee_after.log 2>&1

# 3. REGRESSION: standard close-up camera, no melee seed — must
# sit within the dither floor of light-after100-closeup.png
# (pixel diff, NOT md5).
{
    echo "ARGS: --no-normal-maps --frames 10 --camera -16.2,1.9,0.8:-18.3,1.1,-0.2 --screenshot screenshots/melee-regression.png"
    echo "CAMERA: eye (-16.2,1.9,0.8) look-at (-18.3,1.1,-0.2) — the light-after100 close-up camera"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 10 \
        --camera -16.2,1.9,0.8:-18.3,1.1,-0.2 \
        --screenshot screenshots/melee-regression.png 2>&1
    echo "EXIT: $? $(date -u)"
} > melee_regression.log 2>&1

echo "renders done"
grep -h "melee-test:" melee_before.log melee_after.log
grep -h "killed\|Hit Orgnar" melee_after.log | tail -3
grep -c "Hit Orgnar" melee_after.log
