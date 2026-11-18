#!/bin/sh
# Regenerates the fixtures of tests/gnuindex.rs (WP-251: spec/plumb.md
# §3.3 M10's row for incr.md IN-55 option (A)), for AArch64 and x86-64,
# with the mortar given (WP-229 or later):
#   inc:    a.o b.o p.o, --incremental=full (the GNU sets left out, the
#           -gpubnames sets kept)
#   plain:  the same, no --incremental (every set placed)
#   sorted: the same, --incremental --sort-common (plain by IN-12)
#   group:  a.o b.o x-group.o, --incremental=full (the set of a COMDAT group
#           with an allocated member)
#   alloc:  a.o b.o x-alloc.o, --incremental=full (an allocated section of the
#           name: placed)
# The outputs and maps are checked in: the tests judge them whatever a
# later mortar does. Run from this directory:  sh gen.sh <mortar>
set -e
mortar=$1
for arch in aarch64 x86_64; do
  cc=$arch-linux-gnu-gcc
  mkdir -p "$arch"
  "$cc" -c -O0 -g -gsplit-dwarf -gdwarf-5 a.c -o "$arch/a.o"
  "$cc" -c -O0 -g -gsplit-dwarf -gdwarf-4 b.c -o "$arch/b.o"
  "$cc" -c -O0 -g -gpubnames p.c -o "$arch/p.o"
  "$cc" -c -DGROUP x.S -o "$arch/x-group.o"
  "$cc" -c -DALLOC x.S -o "$arch/x-alloc.o"
  cd "$arch"
  "$mortar" -static --incremental=full a.o b.o p.o -o inc --mortar-map=inc.map
  "$mortar" -static a.o b.o p.o -o plain --mortar-map=plain.map
  "$mortar" -static --incremental --sort-common a.o b.o p.o -o sorted --mortar-map=sorted.map
  "$mortar" -static --incremental=full a.o b.o x-group.o -o group --mortar-map=group.map
  "$mortar" -static --incremental=full a.o b.o x-alloc.o -o alloc --mortar-map=alloc.map
  rm -f a.dwo b.dwo inc.mortar-state group.mortar-state alloc.mortar-state
  cd ..
done
