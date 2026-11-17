#!/bin/sh
# Regenerates WP-144's fixtures of tests/shared.rs
# (`p_d26_mortar_x64_tls_libraries`): mortar's x86-64 shared objects of
# ../tls.c in three TLS dialects (gnu2 descriptors, traditional GD/LD,
# initial-exec) and of gd0.c at -O0 (a general-dynamic pair against a
# local symbol: one DTPMOD64 with r_sym 0 and the static DTPOFF word),
# against a stub libtlso.so (../tlso.c). -nostdlib: every input is here.
#   sh gen.sh <mortar>
# Run from this directory; the outputs are checked in. First made with
# WP-127 part 2's mortar (branch wp/127b-shared-x64-tls, 3ae1238).
set -e
mortar=$1
cc=x86_64-linux-gnu-gcc
"$cc" -O0 -fPIC -c gd0.c -o gd0.o
"$cc" -O1 -fPIC -mtls-dialect=gnu2 -c ../tls.c -o desc.o
"$cc" -O1 -fPIC -c ../tls.c -o gd.o
"$cc" -O1 -fPIC -ftls-model=initial-exec -c ../tls.c -o ie.o
"$cc" -shared -nostdlib -fPIC ../tlso.c -o libtlso.so -Wl,-soname,libtlso.so -s
for n in gd0 desc gd ie; do
  "$mortar" -shared -soname libt.so -o "libt-$n.so" "$n.o" libtlso.so --mortar-map="libt-$n.map"
done
