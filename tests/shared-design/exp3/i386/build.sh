#!/bin/sh
# i386/build.sh: the 6 constructed units (anchorgap, boundary, gap2, hodt, x4, c2) for i386
# with GCC 14, Clang 22 and Clang 19 at -O0/-O2, -fPIE/-fno-pic; read by ../i386how.py.
ulimit -c 0
cd /Volumes/AgentWork/mortar-helper/.build/target-wp-110/exp3/i386 || exit 1
for s in anchorgap.c boundary.c gap2.c hodt.c x4.c c2.c; do for O in -O0 -O2; do for m in -fPIE -fno-pic; do
  b=${s%.c}
  i686-linux-gnu-gcc $O $m -c $s -o $b.gcc$O$m.o 2>/dev/null
  clang-22 --target=i686-linux-gnu $O $m -c $s -o $b.clang22$O$m.o 2>/dev/null
  clang-19 --target=i686-linux-gnu $O $m -c $s -o $b.clang19$O$m.o 2>/dev/null
done; done; done
ls *.o | wc -l
