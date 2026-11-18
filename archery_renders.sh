#!/bin/bash
# Archery proof renders: arrow mid-flight (slow arrow) + kill
# (5 shots x 15 dmg = Orgnar's 75 HP) + regression (standard
# close-up vs light-after100-closeup.png).
# Run AFTER the dovah-app bin is built and hardlinked to
# target/debug/dovah-app-archery.
# Camera coords are recorded in each render log (the
# archery-test seed logs its teleport position).
#
# Speed choice: the arrow drops under 7.7 m/s^2 gravity, so a
# 5 m/s arrow sticks in the floor 3 m out. 15 m/s is the
# slowest speed that still reaches Orgnar at 8 m (arrives at
# y ~0.12 m, his feet). Frame 1 (250 ms clamped delta) puts
# it 3.75 m out at y ~1.23 m — mid-flight; frame 2 puts it
# 7.5 m out, about to hit. Real speed (51.4 m/s) crosses the
# inn in one frame, so the kill render shows the fallen body,
# not the arrow.
#
# CAMERA vs SEED: the camera IS the player entity
# (spawn_camera bundles Camera3d + Player), so the seed's
# teleport overrides the --camera viewpoint — the rendered
# view is ALWAYS the seed's first-person view from 8 m,
# facing Orgnar's chest. --camera only picks the approach
# side (the seed approaches from the spawn's side). The
# z=+4 camera used before put the lane through the tables
# (the arrow stuck at 5.1 m); the z=-4 camera below mirrors
# it to the clear south lane the kill run proved (5/5 hits).
set -u
cd ~/workspace/dovahengine
BIN=./target/debug/dovah-app-archery
FLIGHT_CAM="-14.0,2.4,-4.0:-16.0,1.0,0.3"

# 1. FLIGHT (frame 1): slow arrow mid-flight, first-person
# seed view (the --camera only picks the clear south lane).
{
    echo "ARGS: --no-normal-maps --frames 1 --archery-test 1 --archery-speed 15 --camera $FLIGHT_CAM --screenshot screenshots/archery-flight.png"
    echo "CAMERA: seed first-person view (8m from Orgnar, facing his chest) — --camera only picks the approach side; see the archery-test log line below. Arrow expected ~3.75 m out, y ~1.23 m, dead centre"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 1 --archery-test 1 --archery-speed 15 \
        --camera "$FLIGHT_CAM" \
        --screenshot screenshots/archery-flight.png 2>&1
    echo "EXIT: $? $(date -u)"
} > archery_flight.log 2>&1

# 2. FLIGHT frame 2: the same arrow 7.5 m out, about to hit.
{
    echo "ARGS: --no-normal-maps --frames 2 --archery-test 1 --archery-speed 15 --camera $FLIGHT_CAM --screenshot screenshots/archery-flight-f2.png"
    echo "CAMERA: same seed first-person view as archery-flight.png; arrow expected ~7.5 m out, at Orgnar's feet"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 2 --archery-test 1 --archery-speed 15 \
        --camera "$FLIGHT_CAM" \
        --screenshot screenshots/archery-flight-f2.png 2>&1
    echo "EXIT: $? $(date -u)"
} > archery_flight_f2.log 2>&1

# 3. CONTROL: same camera, same teleport, 0 shots — the
# arrow-free frame for the pixel diff.
{
    echo "ARGS: --no-normal-maps --frames 1 --archery-test 0 --camera $FLIGHT_CAM --screenshot screenshots/archery-flight-control.png"
    echo "CAMERA: same side camera as archery-flight.png; no arrow (0 shots queued)"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 1 --archery-test 0 \
        --camera "$FLIGHT_CAM" \
        --screenshot screenshots/archery-flight-control.png 2>&1
    echo "EXIT: $? $(date -u)"
} > archery_flight_control.log 2>&1

# 4. KILL: 5 queued shots at real speed (51.4 m/s), 30 frames.
# Bow cooldown is 1/0.9375 = 1.067 s = 5 clamped frames per
# shot; shots land on frames 1, 6, 11, 16, 21; Orgnar (75 HP /
# 15 dmg) dies on shot 5. The final frame shows the fallen
# body (same death fall as melee).
{
    echo "ARGS: --no-normal-maps --frames 30 --archery-test 5 --screenshot screenshots/archery-kill.png"
    echo "CAMERA: archery-test seed teleport (8m from Orgnar, facing his chest) — see the archery-test log line below"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 30 --archery-test 5 \
        --screenshot screenshots/archery-kill.png 2>&1
    echo "EXIT: $? $(date -u)"
} > archery_kill.log 2>&1

# 5. REGRESSION: standard close-up camera, no archery seed —
# must sit within the dither floor of
# light-after100-closeup.png (pixel diff, NOT md5).
{
    echo "ARGS: --no-normal-maps --frames 10 --camera -16.2,1.9,0.8:-18.3,1.1,-0.2 --screenshot screenshots/archery-regression.png"
    echo "CAMERA: eye (-16.2,1.9,0.8) look-at (-18.3,1.1,-0.2) — the light-after100 close-up camera"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 10 \
        --camera -16.2,1.9,0.8:-18.3,1.1,-0.2 \
        --screenshot screenshots/archery-regression.png 2>&1
    echo "EXIT: $? $(date -u)"
} > archery_regression.log 2>&1

echo "renders done"
grep -h "archery-test:" archery_flight.log archery_flight_f2.log archery_kill.log
grep -h "Fired\|Shot\|stuck" archery_flight.log archery_flight_f2.log archery_kill.log
