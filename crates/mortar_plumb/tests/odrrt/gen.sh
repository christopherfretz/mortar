#!/bin/sh
# Regenerates the fixtures of tests/odrrt.rs (spec/plumb.md §13.1-§13.2,
# WP-197): mortar's PIE with --odr-runtime=report (and --odr-table=full,
# so .note.mason holds three notes), its shared object with
# --odr-runtime=fatal, and the PIE without the option (=full only), over
# a stand-in libc, for AArch64 and x86-64. The runtime object is not
# copied here: the verifier holds its own (src/odrrt.rs).
#   sh gen.sh <mortar>
# Run from this directory; the outputs are checked in.
set -e
mortar=$1
for arch in aarch64 x86_64; do
  cc=aarch64-linux-gnu-gcc
  [ "$arch" = x86_64 ] && cc=x86_64-linux-gnu-gcc
  command -v "$cc" >/dev/null || cc=gcc
  mkdir -p "$arch"
  cd "$arch"
  "$cc" -c -O1 -fPIE -fno-asynchronous-unwind-tables ../main.c -o main.o
  "$cc" -c -O1 -fPIC -fno-asynchronous-unwind-tables ../lib.c -o lib.o
  "$cc" -shared -nostdlib -fPIC ../c.c -o libc.so.6 -Wl,-soname,libc.so.6 -Wl,-z,max-page-size=4096 -s
  "$mortar" -pie -dynamic-linker /lib/ld.so -o prog main.o libc.so.6 \
    --odr-runtime=report --odr-table=full --mortar-map=prog.map
  "$mortar" -shared -o lib.so lib.o libc.so.6 --odr-runtime=fatal --mortar-map=lib.map
  "$mortar" -pie -dynamic-linker /lib/ld.so -o plain main.o libc.so.6 \
    --odr-table=full --mortar-map=plain.map
  cd ..
done
