#!/bin/sh
# Regenerates WP-149's fixtures of tests/shared.rs
# (`p_d27_mortar_libraries`): mortar's shared objects of ../vd.c under
# vd.map (two nodes, one parent, -soname) and vd3.map (three nodes, the
# last with two parents, no -soname: the base is the output's name), on
# AArch64 and x86-64, against a stub libvdx-<arch>.so (../vdx.c, version
# X1). -nostdlib: every input is here. vd.map and vd3.map are copies of
# ../vd.map and ../vd3.map (the maps name them here).
#   sh gen.sh <mortar>
# Run from this directory; the outputs are checked in. First made with
# the mortar of WP-149's base (int/batch-7, b3eb1da).
set -e
mortar=$1
for a in a64 x64; do
  case $a in a64) cc=aarch64-linux-gnu-gcc;; x64) cc=x86_64-linux-gnu-gcc;; esac
  "$cc" -O1 -fPIC -c ../vd.c -o vd-$a.o
  "$cc" -shared -nostdlib -fPIC ../vdx.c -o libvdx-$a.so -Wl,-soname,libvdx.so \
    -Wl,--version-script,../vdx.map -s -Wl,-z,max-page-size=4096
  "$mortar" -shared -soname libvd.so -o libvd-$a.so vd-$a.o libvdx-$a.so \
    --version-script vd.map --mortar-map=libvd-$a.map
  "$mortar" -shared -o libvd3-$a.so vd-$a.o libvdx-$a.so \
    --version-script vd3.map --mortar-map=libvd3-$a.map
done
