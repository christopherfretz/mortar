#!/bin/bash
# WP-255: build the throwaway read-set test, with and without the probe.
#   build.sh  (run from anywhere; writes the final line "build done rc=N")
ulimit -c 0
cd /Volumes/AgentWork/mortar-helper/mortar/.worktrees/wp-140 || exit 1
unset CARGO_TARGET_DIR
source scripts/env.sh
cargo test --release -p mortar --test wp255_readset --features wp255probe --no-run 2>&1 | tail -30
a=${PIPESTATUS[0]}
cargo test --release -p mortar --test wp255_readset --no-run 2>&1 | tail -5
b=${PIPESTATUS[0]}
echo "build done rc=$a/$b"
