#!/bin/bash
export PATH="$HOME/.cargo/bin:$PATH"
cd ~/workspace/dovahengine || exit 1
cargo build -p dovah-app --bins --example drop_bounds_probe > build_bin.log 2>&1
code=$?
echo "build exit $code $(date)" >> build_bin.log
echo "BUILD_DONE exit=$code $(date)" > build_bin_done.txt
