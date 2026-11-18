#!/bin/bash
# WP-255 addendum (F7, F9): the state PATCH of a body edit on real
# format-5 states (protoc no -g, protoc -g, clang -g): its size, blocks,
# and pwrite+fdatasync time against a whole rewrite; the SLOTS counts.
# States, outputs and the copies deleted by literal name.
ulimit -c 0
S=/Volumes/AgentWork/mortar-helper/.build/target-wp-140/wp255
B=/Volumes/AgentWork/mortar-helper/.build/target-wp-140/release
quiet() { while awk -v l="$(cut -d' ' -f1 /proc/loadavg)" 'BEGIN{exit !(l>12)}'; do sleep 20; done; }
quiet
(
  cd /Volumes/AgentWork/mortar-helper/mortar/.worktrees/wp-140 || exit 1
  unset CARGO_TARGET_DIR; source scripts/env.sh
  cargo test --release -p mortar_incr --test wp255_patch --no-run 2>&1 | grep -E '^(error|warning: unused)|could not compile|Executable' 
  cargo build --release -p mortar --features testhooks --bin mortar 2>&1 | tail -2
  cp $B/mortar $S/mortar-th
) > $S/series6-build.log 2>&1 || { echo "series6 done FAIL build"; exit 1; }
grep -q "could not compile" $S/series6-build.log && { echo "series6 done FAIL build"; exit 1; }
one() { # workload member tag
  wl=$1; mem=$2; out=$S/pt-$3; mkdir -p $out
  W=/Volumes/AgentWork/mortar-helper/.build/bench-work/workloads/$wl
  cwd=$(awk '$1=="cwd"{print $2}' "$W/stamp")
  awk -v o="$out/out" 'prev=="-o"{print o; prev=""; next} /^--dependency-file=/{next} {print; prev=$0}' "$W/response.txt" > "$out/response.txt"
  quiet
  ( cd "$cwd" && MORTAR_INCR_STATE5="$out/s.state5" "$S/mortar-th" @"$out/response.txt" --incremental=full --incremental-state "$out/s.mortar-state" ) > $out/link.log 2>&1
  ls -la $out >> $out/link.log
  cd $out && rm -f out s.mortar-state
  quiet
  echo "load $(cut -d' ' -f1 /proc/loadavg)" > $out/patch.log
  (
    cd /Volumes/AgentWork/mortar-helper/mortar/.worktrees/wp-140 || exit 1
    unset CARGO_TARGET_DIR; source scripts/env.sh
    WP255_STATE=$out/s.state5 WP255_MEMBER=$mem WP255_REPS=9 WP255_WORK=$out \
      cargo test --release -p mortar_incr --test wp255_patch -- --ignored --nocapture --test-threads=1
  ) >> $out/patch.log 2>&1
  echo "load $(cut -d' ' -f1 /proc/loadavg)" >> $out/patch.log
  cd $out && rm -f s.state5 patch.state5 whole.state5
}
one cxx-medium-static strutil.cc.o p
one cxx-medium-g-static strutil.cc.o g
one cxx-large-g StringRef.cpp.o l
echo "series6 done"
