#!/bin/bash
# v0.4.6 phone-feedback renders, sequential (one lavapipe render
# at a time — two OOM the 6 GB box). Relaunched after the second
# VM restart (2026-11-16 ~00:57 UTC) killed render 1 mid-scene-build.
# Waits for lavapipe first: the restart wipes mesa-vulkan-drivers
# and the runtime reconcile restores it; without it every render
# fails with "Unable to find a GPU!".
cd ~/workspace/dovahengine || exit 1
rm -f renders_done.txt
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app

for i in $(seq 1 90); do
    [ -f /usr/share/vulkan/icd.d/lvp_icd.json ] && break
    sleep 10
done
if [ ! -f /usr/share/vulkan/icd.d/lvp_icd.json ]; then
    echo "LAVAPIPE STILL MISSING after 15 min — renders cannot run" > renders_done.txt
    exit 1
fi
echo "lavapipe present, starting renders $(date)"

$BIN --exterior 5 -11 --door-open 13424 --screenshot screenshots/inn-return-door-open.png --no-normal-maps --frames 10 > render_door_open.log 2>&1
echo "door-open exit $? $(date)" >> render_door_open.log

$BIN --exterior 5 -11 --camera 264,-0.3,668:264,-1.0,673 --screenshot screenshots/v046-guard-clothed.png --no-normal-maps --frames 10 > render_guard.log 2>&1
echo "guard exit $? $(date)" >> render_guard.log

$BIN --exterior 5 -11 --camera 332,2,635:300,5,600 --screenshot screenshots/v046-waterfall-north.png --no-normal-maps --frames 10 > render_waterfall.log 2>&1
echo "waterfall exit $? $(date)" >> render_waterfall.log

$BIN --screenshot screenshots/v046-inn-regression.png --no-normal-maps --frames 10 > render_inn.log 2>&1
echo "inn exit $? $(date)" >> render_inn.log

$BIN --screenshot-ui --screenshot screenshots/v046-hud-ingame.png --no-normal-maps --frames 10 > render_hud.log 2>&1
echo "hud exit $? $(date)" >> render_hud.log

echo "ALL RENDERS DONE $(date)" > renders_done.txt
