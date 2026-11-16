#!/bin/sh
# Regenerates the stage-4 fixtures of tests/meta.rs (spec/plumb.md §13;
# WP-078): two objects with test-producer units (`cargo xtask meta`, the
# descriptions a.meta and b.meta), a stub library, and mortar's PIE of
# them with --odr-table=full and --keep-mason-meta, for AArch64 and x86-64.
#   sh gen.sh <mortar> <xtask>
# Run from this directory; the outputs are checked in.
set -e
mortar=$1
xtask=$2
for arch in aarch64 x86_64; do
  cc=aarch64-linux-gnu-gcc
  [ "$arch" = x86_64 ] && cc=x86_64-linux-gnu-gcc
  command -v "$cc" >/dev/null || cc=gcc
  mkdir -p "$arch"
  cd "$arch"
  "$cc" -c -O0 -fPIC -fno-asynchronous-unwind-tables ../a.cc -o a0.o
  "$cc" -c -O0 -fPIC -fno-asynchronous-unwind-tables ../b.cc -o b0.o
  "$cc" -shared -nostdlib -fPIC ../s.c -o libs.so -Wl,-soname,libs.so -Wl,-z,max-page-size=4096 -s
  "$xtask" meta write ../a.meta --into a0.o -o a.o
  "$xtask" meta write ../b.meta --into b0.o -o b.o
  rm a0.o b0.o
  "$mortar" -pie -export-dynamic -dynamic-linker /lib/ld.so -o prog a.o b.o libs.so \
    --odr-table=full --keep-mason-meta --mortar-map=prog.map
  cd ..
done
