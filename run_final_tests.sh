#!/bin/bash
cd ~/workspace/dovahengine || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
rm -f tests_done.txt
cargo test -p dovah-app --lib > final_tests.log 2>&1
echo "TESTS_DONE exit=$? $(date)" > tests_done.txt
