#!/bin/sh
# Regenerates WP-246's fixtures of tests/symnames.rs: mortar's static
# -nostdlib links of start.c a.c b.c (clang -g), AArch64 and x86-64, each
# linked twice from the same objects:
#   x-<arch>  --incremental=full: an incremental-mode output
#   f-<arch>  a plain link
# with their maps. mut.py (run by tests/symnames.rs into a scratch
# directory, never here) mutates their .symtab. The outputs are checked in.
#   sh gen.sh <mortar>
# Run from this directory. First made with the mortar of WP-246's base
# (a828a3de).
# WP-253: the incremental-mode outputs checked in were made before IN-51's
# one 38-byte table (WP-228 part A) and converted in place by
# ../retable.py; a regeneration with a later mortar has the table already.
set -e
mortar=$1
for a in a64 x64; do
  case $a in
    a64) t=aarch64-linux-gnu;;
    x64) t=x86_64-linux-gnu;;
  esac
  f="--target=$t -O1 -g -fno-pic -fno-asynchronous-unwind-tables -fno-stack-protector"
  for s in start a b; do
    clang $f -c $s.c -o $s-$a.o
  done
  "$mortar" -static -o x-$a start-$a.o a-$a.o b-$a.o --incremental=full \
    --incremental-state=state-$a --mortar-map=x-$a.map
  "$mortar" -static -o f-$a start-$a.o a-$a.o b-$a.o --mortar-map=f-$a.map
done
rm -f state-a64 state-x64
