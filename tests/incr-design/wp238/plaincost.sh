#!/bin/bash
# WP-238 step 1, item 1: what the enumerator refactor costs a PLAIN link.
# plaincost.sh BASE NEW OUTDIR LOG RUNS WORKLOAD...
# Interleaved runs (base, new, base, new, ...) of each bench workload's
# captured link plus --gc-sections, at the load the host has; per run the
# wall time and phase 50's (liveness) time from --time-phases; the
# outputs of base and new compared by cmp. Writes one line per run and a
# FINAL line. Run under a 1-minute load below 12 only.
ulimit -c 0
base=$1; new=$2; out=$3; log=$4; runs=$5; shift 5
resp=/Volumes/AgentWork/mortar-helper/.build/target-wp-125/wp238/resp
work=/Volumes/AgentWork/mortar-helper/.build/bench-work/workloads
: > "$log"
for w in "$@"; do
  cwd=$(grep '^cwd' "$work/$w/stamp" | cut -f2)
  for i in $(seq 1 "$runs"); do
    for which in base new; do
      bin=$base; [ $which = new ] && bin=$new
      while [ "$(cut -d' ' -f1 /proc/loadavg | cut -d. -f1)" -ge 12 ]; do sleep 15; done
      load=$(cut -d' ' -f1 /proc/loadavg)
      t0=$(date +%s%N)
      (cd "$cwd" && "$bin" @"$resp/$w.txt" --gc-sections --time-phases -o "$out/$which.out" 2> "$out/$which.err")
      rc=$?
      t1=$(date +%s%N)
      p50=$(grep -o '50 liveness: wall [0-9.]*' "$out/$which.err" | awk '{print $4}'); peak=$(grep -o 'total: .*peak [0-9.]*' "$out/$which.err" | awk '{print $NF}')
      echo "$w $which run $i rc $rc wall_ms $(( (t1 - t0) / 1000000 )) load $load phase50_ms $p50 peak_mib $peak" >> "$log"
    done
    if ! cmp -s "$out/base.out" "$out/new.out"; then echo "$w run $i OUTPUTS DIFFER" >> "$log"; fi
  done
  head -c 2000 "$out/new.err" >> "$log"
done
echo "FINAL plaincost done" >> "$log"
