#!/bin/bash
# WP-249 step 1, item 4: the verifier (cargo xtask plumb) on mortar's own
# output (the tombstone) and on that output with every import of a dropped
# copy hand-patched to the kept copy's offset ('redirect') and to 0
# ('zero'). GCC 14 -O0 -g3 C, static, the host's target (AArch64 here).
#   plumbprobe.sh OUT [MORTAR]   OUT/plumb-*.txt; last line PLUMB DONE
ulimit -c 0
HERE="$(cd "$(dirname "$0")" && pwd)"
WT="$(cd "$HERE/../../.." && pwd)"
OUT="$1"; MORTAR="${2:-/Volumes/AgentWork/mortar-helper/.build/target-wp-143/release/mortar}"
mkdir -p "$OUT/bm"; OUT="$(cd "$OUT" && pwd)"; ln -sf "$MORTAR" "$OUT/bm/ld"
for v in 4 5; do
  for f in a b c; do gcc-14 -O0 -g3 -gdwarf-$v -c "$HERE/src/$f.c" -o "$OUT/$f$v.o"; done
  O="$OUT/a$v.o $OUT/b$v.o $OUT/c$v.o"
  gcc-14 -static -B"$OUT/bm/" -Wl,--mortar-map="$OUT/m$v.map" $O -o "$OUT/m$v"
  python3 -I "$HERE/macros.py" patch "$OUT/m$v" "$OUT/m$v-redirect" redirect $O
  python3 -I "$HERE/macros.py" patch "$OUT/m$v" "$OUT/m$v-zero" zero $O
done
cd "$WT" || exit 1
unset CARGO_TARGET_DIR
source scripts/env.sh
for v in 4 5; do
  for x in m$v m$v-redirect m$v-zero; do
    cargo xtask plumb "$OUT/$x" --map "$OUT/m$v.map" --map-format mortar > "$OUT/plumb-$x.txt" 2>&1
    echo "$x: plumb rc=$? $(wc -l < "$OUT/plumb-$x.txt") lines"
  done
done
echo "PLUMB DONE"
