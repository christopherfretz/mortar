#!/bin/sh
# The before-the-start probe of dyn.md D-117 (WP-115): c.c (`tab - 1`, the
# 1-based idiom, beside exported data), m.f90 and l.f90 (1-based SAVE arrays
# beside module variables), every -O, -fPIC, through the oracle s1d.py.
# Usage: probe.sh ORACLE_DIR (the directory holding s1d.py and elf.py).
# Writes objects into the current directory; prints one line per object and
# PROBE-DONE.
set -e
ulimit -c 0
O=${1:?oracle directory}
H=$(dirname "$0")
for L in O0 O1 O2 O3 Os; do
  aarch64-linux-gnu-gcc -fPIC -$L -c "$H/c.c" -o c-gcc-a64-$L.o
  clang-22 --target=aarch64-linux-gnu -fPIC -$L -c "$H/c.c" -o c-clang22-a64-$L.o
  clang-19 --target=aarch64-linux-gnu -fPIC -$L -c "$H/c.c" -o c-clang19-a64-$L.o
  aarch64-linux-gnu-gfortran -fPIC -$L -c "$H/m.f90" -o m-gf-a64-$L.o
  aarch64-linux-gnu-gfortran -fPIC -$L -c "$H/l.f90" -o l-gf-a64-$L.o
  x86_64-linux-gnu-gcc -fPIC -$L -c "$H/c.c" -o c-gcc-x64-$L.o
  clang-22 --target=x86_64-linux-gnu -fPIC -$L -c "$H/c.c" -o c-clang22-x64-$L.o
  x86_64-linux-gnu-gfortran -fPIC -$L -c "$H/m.f90" -o m-gf-x64-$L.o
  x86_64-linux-gnu-gfortran -fPIC -$L -c "$H/l.f90" -o l-gf-x64-$L.o
done
for f in *-a64-*.o; do python3 "$O/s1d.py" --hits "$f" | grep -v '^TOTAL'; done
for f in *-x64-*.o; do python3 "$O/s1d.py" --x86 --hits "$f" | grep -v '^TOTAL'; done
echo PROBE-DONE
