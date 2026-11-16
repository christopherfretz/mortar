#!/bin/bash
# WP-058 typical dynamic link: bench.sh <rounds>. args.txt holds GNU ld's
# words of kiln's dynamic line for push_back.pass.cpp (output t.exe).
cd "$(dirname "$0")"
mapfile -t A < args.txt
M=/tmp/mortar-wp058-279433/bin2/mortar
run() { local name=$1; shift; local A2=() a; for a in "${A[@]}"; do [ "$a" = t.exe ] && a=out.$name.$i; A2+=("$a"); done
  local t0=$EPOCHREALTIME; /usr/bin/time -f %M -o rss.$name "$@" "${A2[@]}" >/dev/null 2>&1; local st=$?; local t1=$EPOCHREALTIME
  echo "$name $st $(( ${t1/./} - ${t0/./} )) $(tail -1 rss.$name)"; }
for i in $(seq "$1"); do
  run bfd /usr/bin/ld.bfd; run lld /usr/bin/ld.lld; run mold /usr/bin/mold
  run m $M; run m-t1 $M --threads=1
done
