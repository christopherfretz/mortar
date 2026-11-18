#!/bin/bash
# WP-244: the experiments' binaries, copied to BINDIR: `mortar` (release,
# `testhooks`: MORTAR_INCR_STATE5), `mortar-plain` (release, no features:
# what a user runs), `incr_state` (mortar_incr's example), and `ld` ->
# mortar for `g++ -B`. Run from the worktree root (it sources its
# scripts/env.sh). Final line: "bins done rc=N".
#
#   bins.sh BINDIR
set -u
ulimit -c 0
dir=$1
mkdir -p "$dir"
unset CARGO_TARGET_DIR
source scripts/env.sh
rc=0
cargo build --release -p mortar --bin mortar || rc=1
cp "$CARGO_TARGET_DIR/release/mortar" "$dir/mortar-plain" || rc=1
cargo build --release -p mortar --features testhooks --bin mortar || rc=1
cp "$CARGO_TARGET_DIR/release/mortar" "$dir/mortar" || rc=1
cargo build --release -p mortar_incr --example incr_state || rc=1
cp "$CARGO_TARGET_DIR/release/examples/incr_state" "$dir/incr_state" || rc=1
ln -sf mortar "$dir/ld"
echo "bins done rc=$rc"
