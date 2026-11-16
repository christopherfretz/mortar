#!/bin/bash
# run.sh WORK: reproduce every experiment of the census area into a fresh WORK.
# Part 1 (driver lines), Part 2 (real builds), Part 3 (libtool-shaped and CMake lines).
set -e
ulimit -c 0
[ -n "$1" ] || { echo "usage: run.sh WORK"; exit 2; }
WORK=$(realpath -m "$1")
[ -e "$WORK" ] && { echo "WORK must not exist: $WORK"; exit 2; }
HERE=$(cd "$(dirname "$0")" && pwd)
MORTAR=${MORTAR:-/Volumes/AgentWork/mortar-helper/.build/target-wp-110/release/mortar}
mkdir -p "$WORK"
export LC_ALL=C
echo "part 1: driver lines"
python3 -I -B "$HERE/census.py" "$WORK/p1" "$HERE/src" "$MORTAR" > "$WORK/p1.log" 2>&1
python3 -I -B "$HERE/summary.py" "$WORK/p1" >> "$WORK/p1.log" 2>&1
echo "part 2: real builds"
bash "$HERE/realbuilds.sh" "$WORK/p2" "$MORTAR" > "$WORK/p2.log" 2>&1
echo "part 3: libtool-shaped lines and CMake"
bash "$HERE/tools.sh" "$WORK/p3" "$MORTAR" > "$WORK/p3.log" 2>&1
echo "odd cases A/B (GNU ld links with warnings, the library misbehaves)"
bash "$HERE/odd.sh" "$WORK/odd" > "$WORK/odd.log" 2>&1
echo "survey classification"
python3 -I -B "$HERE/survey.py" /Volumes/AgentWork/mortar-helper/.build/real-builds/results > "$WORK/survey.tsv"
echo "tool versions"
for t in gcc g++ gfortran aarch64-linux-gnu-gcc x86_64-linux-gnu-gcc i686-linux-gnu-gcc clang-19 clang-22 ld.bfd ld.lld-19 ld.lld-22 mold qemu-i386 qemu-x86_64 readelf eu-elflint cmake; do
  printf '%s\t%s\n' "$t" "$($t --version 2>&1 | head -1)"; done > "$WORK/versions.txt"
echo "$MORTAR $(stat -c %s "$MORTAR") bytes" >> "$WORK/versions.txt"
du -sh "$WORK"
echo "done: logs in $WORK (p1/summary/, p2/, p3/, odd.log, survey.tsv, versions.txt)"
