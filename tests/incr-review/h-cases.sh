#!/bin/bash
# The hostile-state cases kept (h-hostile.sh), each by position and by
# identity, each in a fresh directory under ROOT. usage: h-cases.sh ROOT
set -u
here=$(cd "$(dirname "$0")" && pwd)
root=$1
n=0
while read -r name op; do
  [ -z "$name" ] && continue
  for m in pos idn; do
    d="$root/$name-$m"; mkdir -p "$d"
    if [ "$m" = idn ]; then export MORTAR_FLOOR_IDENTITY=1; else unset MORTAR_FLOOR_IDENTITY; fi
    echo "--- $name ($m): $op"
    timeout 120 "$here/h-hostile.sh" "$d" $op 2>&1 | grep -v '^ref:' | sed 's/^/    /'
  done
  n=$((n+1))
done <<'CASES'
H1-sections-permuted-in-file swap-sections 5 6
H2-frag-at-swapped frag-at-swap 0 1
H3-frag-at-past-chunk frag-at 0 0x10
H4-frag-at-overlap frag-at 0 0x2
H5-frag-at-none frag-at 0 0xffffffffffffffff
H6-sections-unplaced-swapped swap-sections 3 7
H7-sections-text-data-swapped swap-sections 1 2
H8-frag-at-key-moved frag-at-key 1 7 5
H9-frag-at-other-file frag-at-key 1 16 0
H10-place-swapped swap-place 5 6
CASES
echo "H-CASES-DONE $n"
