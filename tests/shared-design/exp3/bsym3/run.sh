#!/bin/bash
# bsym3/run.sh (m-6): mold's cell for --dynamic-list with -Bsymbolic-functions, both orders, AArch64
# native; the re-review's bsym/run.sh sources (lib.c, pre.c, host.c, dl.list), GNU ld and lld for comparison.
ulimit -c 0; cd "$(dirname "$0")"; mkdir -p w; find w -mindepth 1 -delete
cd w; bash ../gen.sh; CC=gcc; L22="-fuse-ld=lld -B/Volumes/AgentWork/mortar-helper/.build/review-110-adv/lld22"
$CC -O1 -fPIC -c lib.c -o lib.o; $CC -O1 -fPIC -shared pre.c -o pre.so
for ln in "dynlist|-Wl,--dynamic-list=dl.list" "dynlist+Bsf|-Wl,--dynamic-list=dl.list -Wl,-Bsymbolic-functions" "Bsf+dynlist|-Wl,-Bsymbolic-functions -Wl,--dynamic-list=dl.list" "Bsf|-Wl,-Bsymbolic-functions"; do
 name=${ln%%|*}; fl=${ln#*|}
 for l in bfd lld22 mold; do case $l in bfd) U="";; lld22) U="$L22";; mold) U="-fuse-ld=mold";; esac
  d=$l.$name; mkdir -p "$d"
  if ! $CC $U -shared lib.o $fl -o "$d/lib.so" 2>"$d/err"; then echo "$name $l: REFUSED"; continue; fi
  $CC host.c "$d/lib.so" -o "$d/host" -Wl,-rpath,"$PWD/$d"
  echo "$name $l: $(LD_PRELOAD=$PWD/pre.so "$d/host" 2>&1)"
 done
done
