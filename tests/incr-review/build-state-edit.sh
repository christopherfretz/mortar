#!/bin/bash
# Builds state_edit (the hostile-state editor) against this tree's
# mortar_incr rlib, in the debug target. usage: build-state-edit.sh OUT
set -eu
here=$(cd "$(dirname "$0")" && pwd)
D=${CARGO_TARGET_DIR:?source scripts/env.sh}/debug/deps
lib=$(ls -t "$D"/libmortar_incr-*.rlib | head -1)
rustc --edition 2024 -L "$D" --extern mortar_incr="$lib" -o "$1" "$here/state_edit.rs"
echo "built $1"
