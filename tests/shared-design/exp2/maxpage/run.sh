#!/bin/bash
# maxpage/run.sh (D-123 on a 64 KiB max page): libraries linked with
# -z max-page-size=0x10000 by the three references, loaded on this 4 KiB
# kernel; checks p_align, p_offset == p_vaddr mod p_align, and that the
# RELRO end (PT_GNU_RELRO vaddr+memsz) is 64 KiB aligned, then runs.
# Also a hand-patched copy: GNU ld's 4 KiB library with every PT_LOAD
# p_align rewritten to 0x10000 (the congruence then fails: what P-R6 sees).
ulimit -c 0; cd "$(dirname "$0")"; W=work.run; [ -d $W ] && find $W -mindepth 1 -delete; mkdir -p $W; cd $W  # fresh outputs (mold writes in place)
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
gcc -O1 -fPIC -c ../lib.c -o lib.o; gcc -O1 -c ../host.c -o host.o
for ld in bfd lld mold; do case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
 for mp in 4k 64k; do MP=""; [ $mp = 64k ] && MP="-Wl,-z,max-page-size=0x10000"
  so=$PWD/lib.$ld.$mp.so
  gcc $L -shared $MP -Wl,-z,relro lib.o -o $so; gcc host.o $so -o host.$ld.$mp -Wl,-rpath,$PWD
  python3 -I ../phdr.py $so | sed "s/^/$ld $mp: /"
  echo "$ld $mp: run: $(./host.$ld.$mp 2>&1)"
 done
done
cp lib.bfd.4k.so patched.so; python3 -I ../phdr.py --patch-align 0x10000 patched.so | sed 's/^/patched: /'
gcc host.o $PWD/patched.so -o host.patched -Wl,-rpath,$PWD; echo "patched: run: $(./host.patched 2>&1)"
