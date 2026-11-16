#!/bin/bash
# GCC AArch64 -fPIE object (section-anchor access to its own exported global) in -shared:
# all three link it silently; a -no-pie executable copies g -> two instances.
ulimit -c 0; cd "$(dirname "$0")"; W=work2.$$; mkdir $W; cd $W
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
gcc -O2 -fPIE -c ../a.c -o a.o
for ld in bfd lld mold; do
 case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
 mkdir $ld; gcc $L -shared a.o -o $ld/libg.so && gcc -fno-pic -no-pie ../host.c -o $ld/host-nopie $ld/libg.so -Wl,-rpath,$PWD/$ld && gcc ../host.c -o $ld/host-pie $ld/libg.so -Wl,-rpath,$PWD/$ld
 echo "$ld no-pie: $($ld/host-nopie)   ($(readelf -rW $ld/host-nopie | grep -c COPY) COPY)"; echo "$ld pie:    $($ld/host-pie)"
done
