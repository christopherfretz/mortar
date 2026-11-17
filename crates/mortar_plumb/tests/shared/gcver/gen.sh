#!/bin/sh
# Regenerates WP-149's fixtures of tests/shared.rs (`m10_*`): mortar's
# `-shared --gc-sections` libraries of gc.c on AArch64 and x86-64, under
# loc.map (`gone`, `data_gone` local: mortar collects their sections) and
# exp.map (exported: kept). The outputs are checked in.
#   sh gen.sh <mortar>
# Run from this directory. First made with the mortar of WP-149's base
# (int/batch-7, b3eb1da).
set -e
mortar=$1
for a in a64 x64; do
  case $a in a64) cc=aarch64-linux-gnu-gcc;; x64) cc=x86_64-linux-gnu-gcc;; esac
  "$cc" -O1 -fPIC -ffunction-sections -fdata-sections -c gc.c -o gc-$a.o
  for v in loc exp; do
    "$mortar" -shared --gc-sections -o $v-$a.so gc-$a.o --version-script $v.map \
      --mortar-map=$v-$a.map
  done
done
