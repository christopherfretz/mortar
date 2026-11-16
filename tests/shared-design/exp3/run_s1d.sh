#!/bin/sh
# run_s1d.sh: S1d's reference analysis (s1d.py, flow mode) over every corpus list
# of lists/ (four at a time); out/s1d.<list>.txt; then the twin detection tables
# (gt3.py) into out/gt.<pairs>.txt.  Prints ALL-DONE last.
ulimit -c 0
cd /Volumes/AgentWork/mortar-helper/.build/target-wp-110/exp3
mkdir -p out
ls lists/*.lst | xargs -P4 -n1 sh -c 'n=$(basename $1 .lst); python3 s1d.py --hits $(cat $1) > out/s1d.$n.txt 2>&1' sh
ls pairs/*.txt | xargs -P4 -n1 sh -c 'n=$(basename $1 .txt); python3 gt3.py $n @$1 > out/gt.$n.txt 2>&1' sh
echo ALL-DONE
