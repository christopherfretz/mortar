#!/bin/bash
# -Bsymbolic-functions / --dynamic-list / -Bno-symbolic: which symbol TYPES stay preemptible
# (P = interposed by LD_PRELOAD, L = bound inside the library). AArch64 native.
ulimit -c 0; cd "$(dirname "$0")"; W=work.$$; mkdir $W; cd $W
gcc -O1 -fPIC -c ../lib.c -o lib.o; gcc -O1 -fPIC -shared ../pre.c -o pre.so; gcc -O1 -c ../host.c -o host.o
for ld in bfd lld mold; do
 case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B/Volumes/AgentWork/mortar-helper/.build/review-110-adv/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
 while IFS='|' read -r name opts; do
  d=$ld.$name; mkdir $d
  if gcc $L -shared lib.o -o $d/libt.so $opts 2>$d/err; then
   gcc host.o -o $d/host -L$d -ltt 2>/dev/null || gcc host.o $d/libt.so -o $d/host -Wl,-rpath,$PWD/$d
   r=$(LD_PRELOAD=$PWD/pre.so $d/host 2>&1); f=$(readelf -dW $d/libt.so | grep -oE 'SYMBOLIC|FLAGS[^)]*\) .*' | tr '\n' ' ')
   printf '%-5s %-28s %s   [%s]\n' $ld "$name" "$r" "$f"
  else printf '%-5s %-28s LINK FAIL: %s\n' $ld "$name" "$(head -1 $d/err)"; fi
 done <<'LIST'
none|
Bsymbolic-functions|-Wl,-Bsymbolic-functions
Bsymbolic|-Wl,-Bsymbolic
dynlist|-Wl,--dynamic-list=../dl.list
Bsymfunc+dynlist|-Wl,-Bsymbolic-functions,--dynamic-list=../dl.list
dynlist+Bsymfunc|-Wl,--dynamic-list=../dl.list,-Bsymbolic-functions
Bsym+Bno-symbolic|-Wl,-Bsymbolic,-Bno-symbolic
Bsymfunc+Bsym|-Wl,-Bsymbolic-functions,-Bsymbolic
Bsym+Bsymfunc|-Wl,-Bsymbolic,-Bsymbolic-functions
dynlist-data|-Wl,--dynamic-list-data
LIST
done
