#!/bin/sh
# Regenerates WP-210's fixtures of tests/ehlocal.rs (spec/plumb.md §6.5
# Y6, synth.md S-50 and S-76): eh-<arch>.s's hand-made .eh_frame linked
# by mortar, -static --gc-sections (the dead function's FDE removed),
# AArch64 and x86-64. The outputs and maps are checked in.
#   sh gen.sh <mortar>
# Run from this directory. First made with the mortar of WP-210's base
# (1bfce906).
set -e
mortar=$1
for a in a64 x64; do
  case $a in
    a64) cc=aarch64-linux-gnu-gcc;;
    x64) cc=x86_64-linux-gnu-gcc;;
  esac
  $cc -c eh-$a.s -o eh-$a.o
  "$mortar" -static --gc-sections -o eh-$a eh-$a.o --mortar-map=eh-$a.map
done
