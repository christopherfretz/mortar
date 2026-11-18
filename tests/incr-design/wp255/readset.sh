#!/bin/bash
# WP-255: the read set of U-6's refill on a real workload, one archive
# member edited (same size; the layout replayed), and its timing.
#   readset.sh WORKLOAD ARCHIVE MEMBER TAG [probe|time] [REPS] [NTH]
# WORKLOAD: a bench workload name (bench-work/workloads/NAME); ARCHIVE as
# the response file names it; writes SCRATCH/TAG/{readset.tsv,log}; the
# outputs prev/want and the edited archive are deleted by literal name.
ulimit -c 0
wl=$1; ar=$2; mem=$3; tag=$4; mode=${5:-probe}; reps=${6:-1}; nth=${7:-0}
S=/Volumes/AgentWork/mortar-helper/.build/target-wp-140/wp255
W=/Volumes/AgentWork/mortar-helper/.build/bench-work/workloads/$wl
out=$S/$tag
mkdir -p "$out"
cwd=$(awk '$1=="cwd"{print $2}' "$W/stamp")
python3 /Volumes/AgentWork/mortar-helper/mortar/.worktrees/wp-140/tests/incr-design/wp255/edit_member.py "$cwd/$ar" "$mem" "$out/edited.a" "$nth" > "$out/edit.txt" || { echo "readset done FAIL edit"; exit 1; }
feat=""; [ "$mode" = probe ] && feat="--features wp255probe"
while awk -v l="$(cut -d' ' -f1 /proc/loadavg)" 'BEGIN{exit !(l>12)}'; do sleep 20; done
echo "load at start $(cut -d' ' -f1 /proc/loadavg)" > "$out/load.txt"
(
  cd /Volumes/AgentWork/mortar-helper/mortar/.worktrees/wp-140 || exit 1
  unset CARGO_TARGET_DIR
  source scripts/env.sh
  WP255_WL=$W WP255_OUT=$out WP255_ARCHIVE=$ar WP255_EDITED=$out/edited.a WP255_MEMBER=$mem WP255_REPS=$reps MORTAR_STEPS=1 \
    cargo test --release -p mortar --test wp255_readset $feat -- --ignored --nocapture --test-threads=1
) > "$out/log" 2>&1
rc=$?
echo "load at end $(cut -d' ' -f1 /proc/loadavg)" >> "$out/load.txt"
cd "$out" && rm -f prev want edited.a
echo "readset done $tag rc=$rc"
