#!/bin/bash
# WP-110 area `export`: reproduces every experiment into a fresh directory WORK.
# usage: run.sh WORK [PART...]   (parts: A B C D F G; default all; E is derived from D)
set -u
ulimit -c 0   # qemu writes ~11 MB core files for every crashing run otherwise
WORK=$1; shift
PARTS=${*:-A B C D F G}
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$WORK"
{
  for t in gcc x86_64-linux-gnu-gcc i686-linux-gnu-gcc clang-19 clang-22 ld.bfd x86_64-linux-gnu-ld \
           i686-linux-gnu-ld ld.lld-19 ld.lld-22 mold qemu-x86_64 qemu-i386 readelf; do
    printf '%s: ' "$t"; $t --version 2>&1 | head -1
  done
  ldd --version | head -1
  echo "clang cross sysroot: clang --target=<triple> finds /usr/lib/gcc-cross/<triple>/14 and /usr/<triple> by itself:"
  clang-22 --target=x86_64-linux-gnu -v -c -x c /dev/null -o /dev/null 2>&1 | grep 'Selected GCC'
  clang-22 --target=i686-linux-gnu -v -c -x c /dev/null -o /dev/null 2>&1 | grep 'Selected GCC'
  echo "lld is chosen with gcc -B WORK/bin/lldNN -fuse-ld=lld (WORK/bin/lldNN/ld.lld -> /usr/bin/ld.lld-NN)"
} > "$WORK/versions.log" 2>&1
for p in $PARTS; do
  echo "== part $p $(date +%T)"
  python3 -I "$HERE/part$p.py" "$WORK" > "$WORK/part$p.stdout" 2>&1
  echo "   rc=$? (summary: $WORK/$p/summary.txt)"
done
