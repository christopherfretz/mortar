#!/bin/sh
# Regenerates the fixtures of tests/emerge.rs (WP-147): mortar's outputs
# and maps for globals of an empty SHF_MERGE section, placed whole
# (synth.md S-6) and, after a merged input, at the end of the merged
# chunk (layout.md L-162), for AArch64, x86-64 and i386:
#   em:  get.o em.o ew.o        (gms, wcs, lms and their address words)
#   mid: get.o pre.o emid.o     (gmid after gpre's string)
# The outputs are checked in.
#   sh gen.sh <mortar>
# Run from this directory.
set -e
mortar=$1
for arch in aarch64 x86_64 i386; do
  case $arch in
    aarch64) cc=aarch64-linux-gnu-gcc ;;
    x86_64) cc=x86_64-linux-gnu-gcc ;;
    i386) cc=i686-linux-gnu-gcc ;;
  esac
  cd "$arch"
  for s in get em ew pre emid; do
    "$cc" -c "$s.s" -o "$s.o"
  done
  "$mortar" -static get.o em.o ew.o -e get -o em --mortar-map=em.map
  "$mortar" -static get.o pre.o emid.o -e get -o mid --mortar-map=mid.map
  cd ..
done
