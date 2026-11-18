#!/bin/bash
# WP-239: one incremental-mode full link of a bench workload, then the
# WP-239 counts over its map, output and state.
#
#   runwl.sh MORTAR WORKLOADS_DIR NAME SCRATCH [extra mortar args...]
#
# The workload's response file is copied with `-o` pointing into
# SCRATCH/NAME and `--dependency-file=` dropped (it would write into the
# workload's directory); the link runs in the workload's recorded cwd.
# Writes SCRATCH/NAME/{out,out.map,out.mortar-state,*.txt}. Nothing is
# removed here: the caller deletes the output by its literal path.
set -u
ulimit -c 0
mortar=$1; wl=$2/$3; name=$3; out=$4/$3; shift 4
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$out"
cwd=$(awk '$1=="cwd"{print $2}' "$wl/stamp")
awk -v o="$out/out" 'prev=="-o"{print o; prev=""; next} /^--dependency-file=/{next} {print; prev=$0}' "$wl/response.txt" > "$out/response.txt"
( cd "$cwd" && /usr/bin/time -v "$mortar" @"$out/response.txt" --incremental=full --mortar-map="$out/out.map" "$@" ) > "$out/link.log" 2>&1
echo "link rc=$?" > "$out/rc.txt"
grep -E "Maximum resident|Elapsed" "$out/link.log" >> "$out/rc.txt"
python3 "$here/ehmap.py" "$out/out.map" --per-file "$out/eh.tsv" > "$out/ehmap.txt" 2>&1
python3 "$here/ehbytes.py" "$out/out" "$out/eh.tsv" > "$out/ehbytes.txt" 2>&1
python3 "$here/frags.py" "$out/out.map" "$out/out" --nonalloc --state "$out/out.mortar-state" --per-file "$out/fragpairs.tsv" > "$out/frags.txt" 2>&1
python3 "$here/statecount.py" "$out/out.mortar-state" > "$out/state.txt" 2>&1
ls -l "$out/out" "$out/out.mortar-state" >> "$out/rc.txt"
echo "done $name"
