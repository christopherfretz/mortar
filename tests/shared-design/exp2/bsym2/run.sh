#!/bin/bash
# bsym2/run.sh: the D-112 table (M-3/M-4).  For each option row and each
# symbol cell, P = interposed by an LD_PRELOAD definition (preemptible),
# B = bound inside the library.  AArch64 native, x86-64 under qemu.
# Adapted from the reviewer's .build/review-110-adv/bsymfunc.
ulimit -c 0; cd "$(dirname "$0")"; W=work.run; [ -d $W ] && find $W -mindepth 1 -delete; mkdir -p $W; cd $W  # fresh outputs (mold writes in place)
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
for arch in a64 x64; do
 if [ $arch = a64 ]; then CC=gcc; Q=""; else CC=x86_64-linux-gnu-gcc; Q="qemu-x86_64 -L /usr/x86_64-linux-gnu"; fi
 $CC -O1 -fPIC -fcommon -c ../lib.c -o lib.$arch.o; $CC -O1 -fPIC -fcommon -shared ../pre.c -o pre.$arch.so; $CC -O1 -c ../host.c -o host.$arch.o
 for ld in bfd lld mold; do
  case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
  while IFS='|' read -r name opts; do
   d=$arch.$ld.$name; mkdir -p $d
   if $CC $L -shared lib.$arch.o -o $d/libt.so $opts 2>$d/err; then
    $CC host.$arch.o $d/libt.so -o $d/host -Wl,-rpath,$PWD/$d 2>$d/herr
    if [ -n "$Q" ]; then r=$($Q -E LD_PRELOAD=$PWD/pre.$arch.so $d/host 2>&1 | head -1); else r=$(LD_PRELOAD=$PWD/pre.$arch.so $d/host 2>&1 | head -1); fi
    f=$(readelf -dW $d/libt.so | grep -oE '\(SYMBOLIC\)|\(FLAGS\).*' | tr -s ' ' | tr '\n' ' ')
    printf '%s %-5s %-24s %s   [%s]\n' $arch $ld "$name" "$r" "$f"
   else printf '%s %-5s %-24s LINK FAIL: %s\n' $arch $ld "$name" "$(head -1 $d/err | cut -c1-90)"; fi
  done <<'LIST'
none|
Bsymbolic|-Wl,-Bsymbolic
Bsymbolic-functions|-Wl,-Bsymbolic-functions
dynlist|-Wl,--dynamic-list=../dl.list
dynlist+Bsym|-Wl,--dynamic-list=../dl.list,-Bsymbolic
Bsym+dynlist|-Wl,-Bsymbolic,--dynamic-list=../dl.list
dynlist+Bsymfunc|-Wl,--dynamic-list=../dl.list,-Bsymbolic-functions
Bsymfunc+dynlist|-Wl,-Bsymbolic-functions,--dynamic-list=../dl.list
Bsym+Bno-symbolic|-Wl,-Bsymbolic,-Bno-symbolic
Bsymfunc+Bno-symbolic|-Wl,-Bsymbolic-functions,-Bno-symbolic
Bno-symbolic+Bsym|-Wl,-Bno-symbolic,-Bsymbolic
Bsymfunc+Bsym|-Wl,-Bsymbolic-functions,-Bsymbolic
Bsym+Bsymfunc|-Wl,-Bsymbolic,-Bsymbolic-functions
dynlist+Bno-symbolic|-Wl,--dynamic-list=../dl.list,-Bno-symbolic
dynlist-data|-Wl,--dynamic-list-data
dynlist-cpp-new|-Wl,--dynamic-list-cpp-new
dynlist-cpp-typeinfo|-Wl,--dynamic-list-cpp-typeinfo
LIST
 done
done
