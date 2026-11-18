#!/bin/bash
# v0.4.7 phone-feedback round: proof renders.
# Run AFTER building the app binary (cargo build -p dovah-app).
# Each render: --no-normal-maps --frames 10, desktop lavapipe.
# Check `ps` + `uptime` first; never render while a build links.
set -e
cd ~/workspace/dovahengine
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_MANIFEST_DIR="$PWD/crates/dovah-app"
BIN=./target/debug/dovah-app

# 1. Inn spawn view — ambient before/after vs screenshots/load-test-inn.png
$BIN --no-normal-maps --frames 10 --screenshot screenshots/v047-inn-spawn.png
# 2. Inn counter view (Orgnar's face, white streaks, counter top) vs phone shot 06
$BIN --no-normal-maps --frames 10 --camera -14.5,1.6,-0.3:-18.75,1.6,-0.26 --screenshot screenshots/v047-inn-counter.png
# 3. Inn door view vs phone shot 05
$BIN --no-normal-maps --frames 10 --camera -3.65,1.6,0:-3.65,1.4,6.04 --screenshot screenshots/v047-inn-door.png
# 4. Exterior spawn view (markers + white) vs screenshots/load-test-exterior.png
$BIN --exterior 5 -11 --no-normal-maps --frames 10 --screenshot screenshots/v047-ext-spawn.png
# 5. Exterior bridge view vs phone shot 01
$BIN --exterior 5 -11 --no-normal-maps --frames 10 --camera 347,2,564:347,0,540 --screenshot screenshots/v047-ext-bridge.png
echo "ALL V047 RENDERS DONE"
