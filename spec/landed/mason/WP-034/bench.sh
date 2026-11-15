#!/bin/bash
# bench.sh <rounds>: the typical libc++ test link (push_back.pass.cpp) by each linker, interleaved.
cd "${BASH_SOURCE%/*}"
mapfile -t A < args.txt
M=/tmp/mortar-wp034-1178167/bin/mortar-ea4a0e1
run() { local name=$1; shift; local A2=() a; for a in "${A[@]}"; do [ "$a" = t.exe ] && a=out.$name; A2+=("$a"); done
  local t0=$EPOCHREALTIME; /usr/bin/time -f %M -o rss.$name "$@" "${A2[@]}" >/dev/null 2>&1; local st=$?; local t1=$EPOCHREALTIME
  echo "$name $st $(( ${t1/./} - ${t0/./} )) $(tail -1 rss.$name)"; }
for i in $(seq "$1"); do
  run bfd /usr/bin/ld.bfd; run lld /usr/bin/ld.lld; run lld22 /usr/lib/llvm-22/bin/ld.lld; run mold /usr/bin/mold
  run mortar $M; run mortar-t1 $M --threads=1
done
