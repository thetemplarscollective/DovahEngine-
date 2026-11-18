#!/bin/bash
export PATH="$HOME/.cargo/bin:$PATH"
cd ~/workspace/dovahengine || exit 1
cargo build -p dovah-app --example drop_bounds_probe > build_drop.log 2>&1
code=$?
echo "build exit $code $(date)" >> build_drop.log
echo "BUILD_DONE exit=$code $(date)" > build_drop_done.txt
