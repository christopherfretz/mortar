#!/bin/sh
# Regenerates the fixtures of tests/ehsym.rs (WP-143b): mortar's outputs
# and maps for a global at the end of an input .eh_frame that holds only
# a terminator (synth.md S-48a, S-50 rule 3), for AArch64, x86-64 and
# i386:
#   end:  get.o end.o          (no live record: the terminator alone)
#   rec:  get.o getc.o end.o   (records live, before end.o's)
#   rec2: get.o end.o getc.o   (records live, after end.o's)
# The outputs are checked in: the tests judge them whatever a later
# mortar does with the same inputs.
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
  for s in get getc end; do
    "$cc" -c "$s.s" -o "$s.o"
  done
  "$mortar" -static get.o end.o -e get -o end --mortar-map=end.map
  "$mortar" -static get.o getc.o end.o -e get -o rec --mortar-map=rec.map
  "$mortar" -static get.o end.o getc.o -e get -o rec2 --mortar-map=rec2.map
  cd ..
done
