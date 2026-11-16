#!/bin/bash
# bsym2/tvprobe.sh: lld x86-64 --dynamic-list: the TLS GD pair is half bound
# (DTPMOD64 names the symbol, DTPOFF64 resolved at link time).  Prints the
# values read with the interposer preloaded (lib: tv=4 wtv=5 ptv=14; pre:
# tv=104 wtv=105 ptv=114).  Run after run.sh.
ulimit -c 0; cd "$(dirname "$0")/work.run" || exit 1
x86_64-linux-gnu-gcc -O1 -c ../tvprobe.c -o tvprobe.o
for d in x64.lld.dynlist x64.bfd.dynlist x64.lld.none; do
 x86_64-linux-gnu-gcc tvprobe.o $d/libt.so -o $d/tvprobe -Wl,-rpath,$PWD/$d
 echo "$d: $(qemu-x86_64 -L /usr/x86_64-linux-gnu -E LD_PRELOAD=$PWD/pre.x64.so $d/tvprobe)"
done
x86_64-linux-gnu-gcc -O1 -fPIC -fcommon -shared ../pre2.c -o pre2.x64.so
for d in x64.lld.dynlist x64.bfd.dynlist x64.lld.none x64.lld.dynlist+Bsym; do x86_64-linux-gnu-gcc tvprobe.o $d/libt.so -o $d/tvprobe -Wl,-rpath,$PWD/$d; echo "$d (interposer TLS layout shifted): $(qemu-x86_64 -L /usr/x86_64-linux-gnu -E LD_PRELOAD=$PWD/pre2.x64.so $d/tvprobe)"; done
# pre3: the interposer defines only tv, at a TLS offset other than the library's
x86_64-linux-gnu-gcc -O1 -fPIC -shared ../pre3.c -o pre3.x64.so; readelf -sW pre3.x64.so | grep -E " tv$" | head -1
for d in x64.lld.dynlist x64.bfd.dynlist x64.lld.none; do echo "$d (pre3): $(qemu-x86_64 -L /usr/x86_64-linux-gnu -E LD_PRELOAD=$PWD/pre3.x64.so $d/tvprobe)"; done
readelf -sW x64.lld.dynlist/libt.so | grep -E " tv$" | head -1
