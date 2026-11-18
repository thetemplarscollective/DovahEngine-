#!/bin/bash
# Forge proof renders, part 3: the grindstone wheel rotation
# pair — same camera as part 2's mill-grind (t=2.0, wheel at
# 0 deg), now at t=6.565 = 180 deg of wheel rotation (4.565 s
# x 433.7 deg/s = 1980 deg = 5.5 turns). The frame is fully
# static except the wheel (no water/mill/blade in view), so
# any diff confined to the disc proves the wheel turns.
# Waits for a clear box (builds AND other renders) first.
cd ~/workspace/dovahengine
export CARGO_MANIFEST_DIR=$HOME/workspace/dovahengine/crates/dovah-app
BIN=./target/debug/dovah-app-forge

wait_clear() {
  local name="$1" tries=0
  while pgrep -x "cargo" > /dev/null || pgrep -x "rustc" > /dev/null || pgrep -x "rust-lld" > /dev/null || pgrep -x "cc" > /dev/null || pgrep -x "dovah-app" > /dev/null || pgrep -x "dovah-app-forge" > /dev/null; do
    if [ $tries -eq 0 ]; then echo "=== $name: box busy, waiting ==="; fi
    sleep 60; tries=$((tries+1))
    if [ $tries -gt 60 ]; then echo "=== $name: BOX NEVER CLEARED, aborting ==="; exit 1; fi
  done
}

wait_clear mill-grind-180
echo "=== mill-grind-180: starting $(date +%H:%M:%S) ==="
echo "ARGS: --exterior 5 -11 --camera 290.1,-0.2,614.4:291.03,-0.99,612.11 --screenshot screenshots/forge-mill-grind-180.png --no-normal-maps --frames 10 --sawmill-time 6.565" > forge_render_mill-grind-180.log
echo "START: $(date)" >> forge_render_mill-grind-180.log
"$BIN" --exterior 5 -11 --camera 290.1,-0.2,614.4:291.03,-0.99,612.11 --screenshot screenshots/forge-mill-grind-180.png --no-normal-maps --frames 10 --sawmill-time 6.565 >> forge_render_mill-grind-180.log 2>&1
echo "=== mill-grind-180: exit $? $(date +%H:%M:%S) ==="
md5sum screenshots/forge-mill-grind-180.png
grep -H "water cells" forge_render_mill-grind-180.log
