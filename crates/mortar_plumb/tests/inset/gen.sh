#!/bin/sh
# Regenerates the fixtures of tests/inset.rs (WP-156): the input set of a
# static link whose implicit linker script says `EXTERN(k_fn)`; `k_fn` is
# defined only by the member `k.o` of `libk.a` (which also carries an
# `.init_array` constructor); `j.o`, the archive's other member, is
# reached by nothing. For AArch64 and x86-64:
#   p, p.map:          mortar -static m.o ext.ld libk.a  (mortar at
#                      e1e60cf, which parsed EXTERN and ignored it: k.o
#                      missing, the defect of 2026-11-17)
#   p.gnu, p.gnu.map:  GNU ld, the same line (k.o extracted)
# and the links the mutations start from (r.o refers to k_fn, w.o
# weakly):
#   q:  r.o libk.a           (mortar and GNU ld: k.o extracted)
#   qr: libk.a r.o           (mortar only; GNU ld: a back-reference)
#   qa: r.o --whole-archive libk.a --no-whole-archive (GNU ld only:
#       mortar at e1e60cf refuses --whole-archive; k.o and j.o)
#   qw: w.o libk.a           (both: nothing extracted)
# The outputs are checked in: a later mortar (fixed) links the same line
# differently, and the tests judge these bytes.
#   sh gen.sh <mortar>
# Run from this directory.
set -e
mortar=$1
for arch in aarch64 x86_64; do
  cd "$arch"
  for s in m k j r w; do
    "$arch-linux-gnu-as" "$s.s" -o "$s.o"
  done
  rm -f libk.a
  "$arch-linux-gnu-ar" rcsD libk.a k.o j.o
  "$mortar" -static m.o ext.ld libk.a -o p --mortar-map=p.map
  "$arch-linux-gnu-ld" -static m.o ext.ld libk.a -o p.gnu -Map=p.gnu.map
  "$mortar" -static r.o libk.a -o q --mortar-map=q.map
  "$arch-linux-gnu-ld" -static r.o libk.a -o q.gnu -Map=q.gnu.map
  "$mortar" -static libk.a r.o -o qr --mortar-map=qr.map
  "$arch-linux-gnu-ld" -static r.o --whole-archive libk.a --no-whole-archive -o qa.gnu -Map=qa.gnu.map
  "$mortar" -static w.o libk.a -o qw --mortar-map=qw.map
  "$arch-linux-gnu-ld" -static w.o libk.a -o qw.gnu -Map=qw.gnu.map
  cd ..
done
