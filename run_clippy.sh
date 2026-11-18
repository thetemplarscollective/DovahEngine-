#!/bin/bash
cd ~/workspace/dovahengine || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
rm -f clippy_done.txt
cargo clippy -p dovah-app --all-targets > clippy.log 2>&1
echo "CLIPPY_DONE exit=$? $(date)" > clippy_done.txt
