#!/bin/sh
# Regenerates the fixtures of tests/pexport.rs (WP-147): mortar's outputs
# and maps for globals and a local defined in sections with no place
# (spec/resolve.md §10.1 P-1): `gprop` in .note.gnu.property (consumed),
# `gstk` in .note.GNU-stack, `gex` in an SHF_EXCLUDE section, `gbid` in an
# input build-id note (dropped), none referenced; `gplain` placed. For
# AArch64 and x86-64:
#   lib.so:   -shared plain.o prop.o stk.o defs.o
#   libs.so:  the same with -s (no .symtab)
#   loc:      -static plain.o propl.o -e f  (the local `lprop` in the
#             consumed note, `lpros` in .data)
# The outputs are checked in: the tests judge them whatever a later
# mortar does with the same inputs.
#   sh gen.sh <mortar>
# Run from this directory.
set -e
mortar=$1
for arch in aarch64 x86_64; do
  case $arch in
    aarch64) cc=aarch64-linux-gnu-gcc ;;
    x86_64) cc=x86_64-linux-gnu-gcc ;;
  esac
  cd "$arch"
  for s in plain prop stk defs propl; do
    "$cc" -c "$s.s" -o "$s.o"
  done
  "$mortar" -shared plain.o prop.o stk.o defs.o -o lib.so --mortar-map=lib.map
  "$mortar" -shared -s plain.o prop.o stk.o defs.o -o libs.so --mortar-map=libs.map
  "$mortar" -static plain.o propl.o -e f -o loc --mortar-map=loc.map
  cd ..
done
