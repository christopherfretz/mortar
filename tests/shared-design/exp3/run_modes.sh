#!/bin/sh
# run_modes.sh: s1d.py's variants (linear / reset scope, calls=kill, analysis boundary
# at access only) over the A64 corpora; out/modes.<variant>.<list>.txt. Prints MODES-DONE.
ulimit -c 0
cd /Volumes/AgentWork/mortar-helper/.build/target-wp-110/exp3
for v in --mode=linear --mode=reset --calls=kill --anabnd=access; do
  for l in sys bb-pic author-pie author-pic abseil-pie abseil-pic cx-a64; do echo "$v $l"; done
done | xargs -P4 -n2 sh -c 'python3 s1d.py $1 $(cat lists/$2.lst) > out/modes.${1#--}.$2.txt 2>&1' sh
echo MODES-DONE
