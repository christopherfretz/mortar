#!/bin/bash
# startstop/run.sh (M-5): __start_/__stop_ of a C-identifier section in a
# -shared output: binding/visibility in .dynsym, and whether an interposer
# (LD_PRELOAD defining __start_cid/__stop_cid) changes the library's count.
# AArch64 native, x86-64 under qemu.  Adapted from the reviewer's startstop.
ulimit -c 0; cd "$(dirname "$0")"; W=work.run; [ -d $W ] && find $W -mindepth 1 -delete; mkdir -p $W; cd $W  # fresh outputs (mold writes in place)
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
printf 'int __start_cid[100]; int __stop_cid[1];\n' > pre.c
printf '#include <stdio.h>\nint count(void);\nint main(void){printf("count=%%d\\n",count());return 0;}\n' > host.c
for arch in a64 x64; do
 if [ $arch = a64 ]; then CC=gcc; Q=""; else CC=x86_64-linux-gnu-gcc; Q="qemu-x86_64 -L /usr/x86_64-linux-gnu"; fi
 $CC -O1 -fPIC -c ../lib.c -o lib.$arch.o; $CC -fPIC -shared pre.c -o pre.$arch.so; $CC -c host.c -o host.$arch.o
 for ld in bfd lld mold; do case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
  for gc in nogc gc; do G=""; [ $gc = gc ] && G="-Wl,--gc-sections"
   so=$PWD/lib.$arch.$ld.$gc.so
   $CC $L -shared $G lib.$arch.o -o $so
   $CC host.$arch.o $so -o host.$arch.$ld.$gc
   if [ -n "$Q" ]; then r=$($Q -E LD_PRELOAD=$PWD/pre.$arch.so ./host.$arch.$ld.$gc); else r=$(LD_PRELOAD=$PWD/pre.$arch.so ./host.$arch.$ld.$gc); fi
   echo "$arch $ld $gc: dynsym: $(readelf --dyn-syms -W $so | awk '/__st(art|op)_cid/{print $8"("$5","$6")"}' | tr '\n' ' ') preloaded: $r  section-kept: $(readelf -SW $so | grep -c ' cid ')"
  done
 done
done
