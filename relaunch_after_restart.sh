#!/bin/bash
# Relaunch after the 2026-11-15 VM restart killed the test run +
# both probes (SIGTERM, no output). Runs the three jobs
# SEQUENTIALLY (one cargo lock, no contention between them),
# each to its own log. Launched via nohup; check the logs.
export PATH="$HOME/.cargo/bin:$PATH"
cd ~/workspace/dovahengine || exit 1

{
    echo "=== lib tests start $(date)"
    cargo test -p dovah-app --lib
    echo "=== lib tests exit $? $(date)"
} > lib_test_run2.log 2>&1

{
    echo "=== guard probe start $(date)"
    cargo run -q -p dovah-app --example guard_outfit_probe
    echo "=== guard probe exit $? $(date)"
} > guard_probe_run2.log 2>&1

{
    echo "=== inn door probe start $(date)"
    cargo run -q -p dovah-app --example inn_door_probe
    echo "=== inn door probe exit $? $(date)"
} > inn_door_probe.log 2>&1

echo "ALL DONE $(date)" > relaunch_done.txt
