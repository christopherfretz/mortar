#!/bin/sh
# Regenerates WP-222's fixtures of tests/symfill.rs: mortar's static
# -nostdlib links of start.c a.c b.c (clang -g -fcommon), AArch64 and
# x86-64, each linked twice from the same objects:
#   x-<arch>  --incremental=full: an incremental-mode output (its map says
#             `incremental`, its debug sections carry IN-51's slack)
#   f-<arch>  a plain link
# with their maps. edit.py (run by tests/symfill.rs into a scratch
# directory, never here) turns them into IN-49a's shape and mutates them.
# The outputs are checked in.
#   sh gen.sh <mortar>
# Run from this directory. First made with the mortar of WP-222's base
# (int/batch-12, 294101b1).
set -e
mortar=$1
for a in a64 x64; do
  case $a in
    a64) t=aarch64-linux-gnu;;
    x64) t=x86_64-linux-gnu;;
  esac
  f="--target=$t -O1 -g -fcommon -fno-pic -fno-asynchronous-unwind-tables -fno-stack-protector"
  for s in start a b; do
    clang $f -c $s.c -o $s-$a.o
  done
  "$mortar" -static -o x-$a start-$a.o a-$a.o b-$a.o --incremental=full \
    --incremental-state=state-$a --mortar-map=x-$a.map
  "$mortar" -static -o f-$a start-$a.o a-$a.o b-$a.o --mortar-map=f-$a.map
done
rm -f state-a64 state-x64
