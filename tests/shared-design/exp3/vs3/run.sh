#!/bin/bash
# vs3/run.sh: the third pass's version-script measurements (M-3, M-4, M-5), in order.
ulimit -c 0; cd "$(dirname "$0")"; mkdir -p out ref; find out ref -mindepth 1 -delete   # fresh outputs (mold writes in place)
gcc -c odd.s -o odd.o; gcc -fPIC -c sv3.c -o sv3.o
g++ -fPIC -c lib.cc -o lib.o; g++ -fPIC -c names.cc -o names.o; gcc -fPIC -c sv.c -o sv.o
python3 globs.py > globs.out; python3 dups.py > dups.out
python3 expdyn.py > expdyn.out; python3 atat.py > atat.out
python3 gnuref.py; python3 compare.py > compare.out; tail -1 compare.out
