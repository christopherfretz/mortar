#!/bin/sh
# Regenerates the fixtures of tests/gc_empty.rs (WP-156b): `e.s` has an
# empty section kept (`_start` holds `empty_live`'s address), an empty one
# collected (`empty_dead`) and a non-empty one collected (`full_dead`);
# address words of the three in `.debug_addr`, `.debug_rnglists`,
# `.debug_ranges`, `.debug_loc` and `.debug_info`. For AArch64 and x86-64,
# `-static --gc-sections e.o`:
#   p, p.map:          mortar
#   p.gnu, p.gnu.map:  GNU ld
#   p.lld, p.lld.map:  lld
# and `c.s` assembled twice (`c1.o`, `c2.o`: an empty `.text.cd` in the
# COMDAT group `cd_fn`, words of its section symbol in the same five
# sections), `-static s.o c1.o c2.o` without `--gc-sections`, `c2.o`'s
# copy not kept: c, c.map (mortar), c.gnu, c.gnu.map, c.lld, c.lld.map.
#   sh gen.sh <mortar>
# Run from this directory.
set -e
mortar=$1
for arch in aarch64 x86_64; do
  mkdir -p "$arch"
  "$arch-linux-gnu-as" e.s -o "$arch/e.o"
  "$arch-linux-gnu-as" s.s -o "$arch/s.o"
  "$arch-linux-gnu-as" c.s -o "$arch/c1.o"
  "$arch-linux-gnu-as" c.s -o "$arch/c2.o"
  cd "$arch"
  "$mortar" -static --gc-sections e.o -o p --mortar-map=p.map
  "$arch-linux-gnu-ld" -static --gc-sections e.o -o p.gnu -Map=p.gnu.map
  ld.lld -static --gc-sections e.o -o p.lld -Map=p.lld.map
  "$mortar" -static s.o c1.o c2.o -o c --mortar-map=c.map
  "$arch-linux-gnu-ld" -static s.o c1.o c2.o -o c.gnu -Map=c.gnu.map
  ld.lld -static s.o c1.o c2.o -o c.lld -Map=c.lld.map
  cd ..
done
