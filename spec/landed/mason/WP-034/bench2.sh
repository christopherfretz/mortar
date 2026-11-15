#!/bin/bash
# bench2.sh <rounds>: as bench.sh, mortar at the merged head da61bbc, with and without MORTAR_HUGEPAGES=1.
cd "${BASH_SOURCE%/*}"
mapfile -t A < args.txt
M=/tmp/mortar-wp034-1178167/bin/mortar-da61bbc
run() { local name=$1; shift; local A2=() a; for a in "${A[@]}"; do [ "$a" = t.exe ] && a=out.$name; A2+=("$a"); done
  local t0=$EPOCHREALTIME; /usr/bin/time -f %M -o rss.$name "$@" "${A2[@]}" >/dev/null 2>&1; local st=$?; local t1=$EPOCHREALTIME
  echo "$name $st $(( ${t1/./} - ${t0/./} )) $(tail -1 rss.$name)"; }
for i in $(seq "$1"); do
  run bfd /usr/bin/ld.bfd; run lld /usr/bin/ld.lld; run mold /usr/bin/mold
  run m $M; run m-t1 $M --threads=1; run m-hp env MORTAR_HUGEPAGES=1 $M; run m-t1-hp env MORTAR_HUGEPAGES=1 $M --threads=1
done
