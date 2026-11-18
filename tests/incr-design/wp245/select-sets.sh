#!/bin/bash
# WP-245 experiment: `select_in_per_file_equals_restricted_full_selection`
# (crates/mortar_synth/tests/real.rs) on protoc's objects and on kiln/hod
# objects, through MORTAR_EH_SELECT_SETS.
#  set 0: GCC AArch64 protoc (main.cc.o, libprotoc.a, libprotobuf.a of the
#         bench's cxx-medium-g-static)
#  set 1: GCC x86-64, the first 29 non-test protobuf sources (-O2 -g)
#  set 2: kiln+hod AArch64, the same 29 (-O0 -g)
#  set 3: kiln+hod x86-64, the same 29 (-O0 -g)
# Usage: select-sets.sh <worktree> <scratch dir>; MASON_KILN, MASON_HOD set.
# Writes a FINAL line "select-sets: PASS" or "select-sets: FAIL ...".
set -u
wt=$1 out=$2
bench=/Volumes/AgentWork/mortar-helper/.build/bench-work
pb=$bench/src/4eab9b524aa5913c/protobuf-3.21.12/src
b=$bench/workloads/cxx-medium-g-static/build
mkdir -p "$out/gcc-x64" "$out/kiln-a64" "$out/kiln-x64"
srcs=$(ls "$pb"/google/protobuf/*.cc | grep -v -e test -e mock | head -29)
export KILN_BACKEND=hod KILN_HOD=$MASON_HOD KILN_HOD_STRICT=1 PATH=$(dirname "$MASON_HOD"):$PATH
cc1() { # dir compiler... -- src
  local d=$1; shift
  local s=${@: -1}; set -- "${@:1:$#-1}"
  local o=$d/$(basename "$s" .cc).o
  "$@" -std=c++17 -I"$pb" -c "$s" -o "$o" 2>>"$d/errors.log" || echo "$s" >>"$d/failed.txt"
}
export -f cc1; export pb
for s in $srcs; do
  echo "$out/gcc-x64 x86_64-linux-gnu-g++ -O2 -g $s"
  echo "$out/kiln-a64 $MASON_KILN -O0 -g $s"
  echo "$out/kiln-x64 $MASON_KILN --target=x86_64-linux-gnu -O0 -g $s"
done | xargs -P 4 -L 1 bash -c 'cc1 "$@"' _
for d in gcc-x64 kiln-a64 kiln-x64; do
  echo "$d: $(ls "$out/$d"/*.o 2>/dev/null | wc -l) objects, $(cat "$out/$d/failed.txt" 2>/dev/null | wc -l) failed"
done
j() { ls "$out/$1"/*.o | paste -sd, -; }
s0="$(find "$b" -name main.cc.o | head -1),$b/libprotoc.a,$b/libprotobuf.a"
export MORTAR_EH_SELECT_SETS="$s0;$(j gcc-x64);$(j kiln-a64);$(j kiln-x64)"
cd "$wt" && unset CARGO_TARGET_DIR && source scripts/env.sh && ulimit -c 0
cargo test -q -p mortar_synth --test real select_in_per_file_equals_restricted_full_selection -- --nocapture >"$out/test.log" 2>&1
rc=$?
grep -E 'equal|panicked|test result' "$out/test.log"
if [ $rc = 0 ]; then echo "select-sets: PASS"; else echo "select-sets: FAIL rc=$rc"; fi
