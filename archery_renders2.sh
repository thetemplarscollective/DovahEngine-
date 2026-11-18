#!/bin/bash
# Archery re-shoot: the first flight frame's arrow was 43 px
# (end-on, 4 m away, against the antlers) and the kill frame
# hid the body behind the counter. These two fix the framing.
#
# Geometry (proven by the first runs): with --camera the
# player spawns AT the camera eye, and the seed teleports 8 m
# from Orgnar along the eye direction — so the flight path
# always points at the camera (a side view is impossible
# without decoupling the player from the camera, which is
# architectural and out of scope). What CAN be done: put the
# camera on the same ray, close to the subject.
#
# 1. STUCK CLOSEUP: the 15 m/s arrow sticks at (-16.6, 0.8,
#    1.7) on the ray u=(0.7447, 0.6675) from Orgnar. A camera
#    at Orgnar+5u = (-15.03, 2.4, 3.08) is 2 m from the stick
#    point, looking at it; the teleport (Orgnar+8u) and the
#    path are IDENTICAL to the first flight runs, so the arrow
#    sticks at the same spot. Expected arrow size ~100x30 px.
# 2. KILL FROM ABOVE: camera at (-16.0, 3.5, 2.0) looking down
#    at Orgnar's feet (-18.75, 0.2, -0.26) — from 3.5 m the
#    sightline clears the 1.1 m counter, so the fallen body
#    is visible. The teleport follows the eye (u=(0.772,
#    0.635) -> (-12.57, 4.82)); at real speed the arrows fly
#    nearly flat and clear the table.
set -u
cd ~/workspace/dovahengine
BIN=./target/debug/dovah-app-archery

# 1. STUCK CLOSEUP: 1 shot at 15 m/s, 10 frames (the arrow
# sticks at ~0.34 s and stays stuck).
{
    echo "ARGS: --no-normal-maps --frames 10 --archery-test 1 --archery-speed 15 --camera -15.03,2.4,3.08:-16.6,0.8,1.7 --screenshot screenshots/archery-stuck-closeup.png"
    echo "CAMERA: eye (-15.03,2.4,3.08) look-at (-16.6,0.8,1.7) — 2 m from the known stick point, on the same ray as the first flight runs (teleport and path identical)"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 10 --archery-test 1 --archery-speed 15 \
        --camera -15.03,2.4,3.08:-16.6,0.8,1.7 \
        --screenshot screenshots/archery-stuck-closeup.png 2>&1
    echo "EXIT: $? $(date -u)"
} > archery_stuck_closeup.log 2>&1

# 2. CLOSEUP CONTROL: same camera, 0 shots — the arrow-free
# frame for the pixel diff.
{
    echo "ARGS: --no-normal-maps --frames 10 --archery-test 0 --camera -15.03,2.4,3.08:-16.6,0.8,1.7 --screenshot screenshots/archery-stuck-closeup-control.png"
    echo "CAMERA: same as archery-stuck-closeup.png; no arrow (0 shots queued)"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 10 --archery-test 0 \
        --camera -15.03,2.4,3.08:-16.6,0.8,1.7 \
        --screenshot screenshots/archery-stuck-closeup-control.png 2>&1
    echo "EXIT: $? $(date -u)"
} > archery_stuck_closeup_control.log 2>&1

# 3. KILL FROM ABOVE: 5 shots at real speed, 30 frames, high
# camera looking down at Orgnar's fallen body.
{
    echo "ARGS: --no-normal-maps --frames 30 --archery-test 5 --camera -16.0,3.5,2.0:-18.75,0.2,-0.26 --screenshot screenshots/archery-kill2.png"
    echo "CAMERA: eye (-16.0,3.5,2.0) look-at (-18.75,0.2,-0.26) — above the counter, looking down at Orgnar's position; teleport follows the eye (see header)"
    echo "BIN: $BIN"
    echo "START: $(date -u)"
    "$BIN" --no-normal-maps --frames 30 --archery-test 5 \
        --camera -16.0,3.5,2.0:-18.75,0.2,-0.26 \
        --screenshot screenshots/archery-kill2.png 2>&1
    echo "EXIT: $? $(date -u)"
} > archery_kill2.log 2>&1

echo "reshoot done"
grep -h "archery-test:\|Fired\|Shot\|stuck" archery_stuck_closeup.log archery_kill2.log
