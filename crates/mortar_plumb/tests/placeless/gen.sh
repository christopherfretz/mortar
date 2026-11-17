#!/bin/sh
# Regenerates the fixtures of tests/placeless.rs (WP-143): the two silent
# mislinks of WP-136 as mortar wrote them before its fix, for AArch64,
# x86-64 and i386. Each architecture's directory holds the sources; this
# assembles them and links
#   gms:   get.o emptymerge.o r-gms.o  (a global at offset 0 of an empty
#          SHF_MERGE|SHF_STRINGS section, referenced by an address word)
#   gprop: get.o prop.o r-gprop.o      (a global in an input
#          .note.gnu.property, which the link consumes)
# with the mortar given, writing its map. The outputs are checked in: the
# tests judge them whatever a later mortar does with the same inputs.
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
  for s in get emptymerge r-gms prop r-gprop; do
    "$cc" -c "$s.s" -o "$s.o"
  done
  "$mortar" -static get.o emptymerge.o r-gms.o -e get -o gms --mortar-map=gms.map
  "$mortar" -static get.o prop.o r-gprop.o -e get -o gprop --mortar-map=gprop.map
  cd ..
done
