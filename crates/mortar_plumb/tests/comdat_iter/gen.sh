#!/bin/sh
# Regenerates the fixtures of tests/comdat_iter.rs (WP-231): mortar's
# static links of WP-200's cells (spec/landed/resolve/WP-200.md §3), the
# objects assembled from the sources here for AArch64 and x86-64 (`-nostdlib`
# links; never run). wa.o keeps COMDAT group g; wb.o's copy (with a weak
# `hx` in it) is dropped; wu.o refers to `hx` strongly; libhx.a's member
# defines it:
#   ext:    wu.o wa.o wb.o libhx.a           (hx.o extracted, iteration 2)
#   after:  wa.o wb.o wu.o libhx.a           (the same)
#   member: wu.o wa.o libwb.a libhx.a        (the dropped copy in a member)
#   copy:   wu.o wa.o wb.o libhxg.a          (the member has a copy of g too)
#   grp:    wu.o --start-group libwa2.a wb2.o --end-group libhx.a
#           (the keeper wa2.o extracted after wb2.o's copy is read)
#   dem:    wu2.o wa.o wbs.o libhx.a         (wbs.o's `hx` is GLOBAL in its
#           dropped copy and nothing refers to `hx`: nothing extracted)
# mortar is WP-200's (the branch wp/200-strong-ref-extracts at 99a208db).
#   sh gen.sh <mortar>
# Run from this directory.
set -e
mortar=$1
for arch in aarch64 x86_64; do
  cd "$arch"
  for s in wu wu2 wa wb wbs hx hxg wa2 wb2; do
    "$arch-linux-gnu-as" "$s.s" -o "$s.o"
  done
  rm -f libhx.a libwb.a libhxg.a libwa2.a
  "$arch-linux-gnu-ar" rcsD libhx.a hx.o
  "$arch-linux-gnu-ar" rcsD libwb.a wb.o
  "$arch-linux-gnu-ar" rcsD libhxg.a hxg.o
  "$arch-linux-gnu-ar" rcsD libwa2.a wa2.o
  "$mortar" -static wu.o wa.o wb.o libhx.a -o ext --mortar-map=ext.map
  "$mortar" -static wa.o wb.o wu.o libhx.a -o after --mortar-map=after.map
  "$mortar" -static wu.o wa.o libwb.a libhx.a -o member --mortar-map=member.map
  "$mortar" -static wu.o wa.o wb.o libhxg.a -o copy --mortar-map=copy.map
  "$mortar" -static wu.o --start-group libwa2.a wb2.o --end-group libhx.a -o grp --mortar-map=grp.map
  "$mortar" -static wu2.o wa.o wbs.o libhx.a -o dem --mortar-map=dem.map
  cd ..
done
