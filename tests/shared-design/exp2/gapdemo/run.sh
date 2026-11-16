#!/bin/bash
# gapdemo/run.sh: the GCC AArch64 section-anchor gap (B-1 / B-2 point 6).
# gap2.c compiled the GCC default (-fPIE) at -O2 into a -shared library; a
# -no-pie host copy-relocates 'g'.  Correct: all=107 g=7.  Three linkers, a64
# natively; x86-64 under qemu (GCC x86-64 -fPIE names 'g' by symbol: D-117).
ulimit -c 0; cd "$(dirname "$0")"; W=work.run; [ -d $W ] && find $W -mindepth 1 -delete; mkdir -p $W; cd $W  # fresh outputs (mold writes in place)
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
for arch in a64 x64; do
 if [ $arch = a64 ]; then CC=gcc; RUN=""; else CC=x86_64-linux-gnu-gcc; RUN="qemu-x86_64 -L /usr/x86_64-linux-gnu"; fi
 $CC -O2 -c ../gap2.c -o gap2.$arch.o
 $CC -O2 -c ../host.c -o host.$arch.o -fno-pie
 echo "== $arch relocations naming g's bytes:"; readelf -rW gap2.$arch.o | grep -E "\.data|\.bss| g\b" | awk '{print "   ",$3,$5,$6,$7}'
 for ld in bfd lld mold; do
  case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
  [ $arch = x64 ] && [ $ld = lld ] && L="-B$R/lld22 -fuse-ld=lld"
  so=lib$arch$ld.so
  if $CC $L -shared gap2.$arch.o -o $so 2>$so.err; then
    if $CC $L -no-pie host.$arch.o ./$so -o host.$arch.$ld -Wl,-rpath,'$ORIGIN' 2>host.$arch.$ld.err; then
      r=$(cd . && $RUN ./host.$arch.$ld 2>&1); echo "$arch $ld: $r"
    else echo "$arch $ld: host link FAIL: $(head -1 host.$arch.$ld.err)"; fi
  else echo "$arch $ld: -shared REFUSED: $(head -1 $so.err)"; fi
 done
done
