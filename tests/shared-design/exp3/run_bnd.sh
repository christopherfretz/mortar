#!/bin/sh
# run_bnd.sh: the address rule's boundary variants (m-1): --boundary=always (every
# one-past-the-end address that starts an exported definition is a bound use) and
# --boundary=never (none is), against the refined default in out/s1d.*; out/bnd.<v>.<list>.txt.
ulimit -c 0
cd /Volumes/AgentWork/mortar-helper/.build/target-wp-110/exp3
for v in always never; do
  for l in sys bb-pic author-pie author-pic abseil-pie abseil-pic cx-a64; do echo "$v $l"; done
done | xargs -P4 -n2 sh -c 'python3 s1d.py --boundary=$1 $(cat lists/$2.lst) > out/bnd.$1.$2.txt 2>&1' sh
echo BND-DONE
