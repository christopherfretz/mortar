#!/bin/bash
# D-110 item 2: are __start_/__stop_ of a C-identifier section exported from -shared by the references?
ulimit -c 0; cd "$(dirname "$0")"; W=work.$$; mkdir $W; cd $W
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
gcc -O1 -fPIC -c ../lib.c -o lib.o
for ld in bfd lld mold; do case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
 gcc $L -shared lib.o -o lib.$ld.so; echo "$ld: dynsym: $(readelf --dyn-syms -W lib.$ld.so | awk '/__st(art|op)_cid/{print $8"("$4","$5","$6","$7")"}' | tr '\n' ' ')"; done
