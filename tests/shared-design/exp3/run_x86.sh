#!/bin/sh
# run_x86.sh: D-117's address rule under the M-1 scope (exact fields only on x86; s1d.py --x86)
# over the x86 corpora; out/x86rule.<list>.txt.  Prints X86-DONE.
ulimit -c 0
cd /Volumes/AgentWork/mortar-helper/.build/target-wp-110/exp3
ls lists/x-*.lst | xargs -P4 -n1 sh -c 'n=$(basename $1 .lst); python3 s1d.py --x86 --hits $(cat $1) | awk "/^[^ ]/{s=(\$0 !~ /rule=0 ana=0/)} s" > out/x86rule.$n.txt 2>&1' sh
echo X86-DONE
