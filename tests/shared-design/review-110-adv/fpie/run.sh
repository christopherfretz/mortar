#!/bin/bash
# -fPIE (and -fno-pic) objects into -shared: which link, per arch, compiler, linker.
# D-122 claims "AArch64 and i386 -fPIE code links as -fPIC (all three)".
ulimit -c 0; cd "$(dirname "$0")"; W=work.$$; mkdir $W; cd $W
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
for arch in aarch64 x86_64; do
 if [ $arch = aarch64 ]; then GCC=gcc; CL="clang-22"; else GCC=x86_64-linux-gnu-gcc; CL="clang-22 --target=x86_64-linux-gnu"; fi
 for cc in gcc clang; do for pic in -fPIE -fno-pic; do
  for t in a b c d e s; do
   o=$arch.$cc$pic.$t.o
   if [ $cc = gcc ]; then $GCC -O1 $pic -c ../$t.c -o $o; else $CL -O1 $pic -c ../$t.c -o $o; fi
   line="$arch $cc $pic $t:"
   for ld in bfd lld mold; do
    case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
    if $GCC $L -shared -nostdlib $o -o $o.$ld.so 2>$o.$ld.err; then
      tr=$(readelf -dW $o.$ld.so | grep -c TEXTREL); line="$line $ld=ok$( [ $tr -gt 0 ] && echo +TEXTREL)"
    else line="$line $ld=REFUSED"; fi
   done
   echo "$line   relocs: $(readelf -rW $o | awk '/R_/{print $3}' | sort -u | tr '\n' ' ')"
  done
 done; done
done
