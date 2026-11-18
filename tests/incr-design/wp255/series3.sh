#!/bin/bash
# WP-255: the mutations (m1, m2) on protoc, and the state primitives:
# a format-5 state of protoc (no -g, -g) by the testhooks CLI, then
# wp255_state's timed reads; the states and outputs deleted by literal name.
ulimit -c 0
S=/Volumes/AgentWork/mortar-helper/.build/target-wp-140/wp255
B=/Volumes/AgentWork/mortar-helper/.build/target-wp-140/release
R=/Volumes/AgentWork/mortar-helper/mortar/.worktrees/wp-140/tests/incr-design/wp255/readset.sh
(
  cd /Volumes/AgentWork/mortar-helper/mortar/.worktrees/wp-140 || exit 1
  unset CARGO_TARGET_DIR; source scripts/env.sh
  cargo test --release -p mortar --test wp255_state --no-run 2>&1 | tail -2
  # A `cargo test` rebuilds the binary without the feature: the testhooks
  # binary is built last and copied aside.
  cargo build --release -p mortar --features testhooks --bin mortar 2>&1 | tail -2
  cp $B/mortar $S/mortar-th
) > $S/series3-build.log 2>&1
[ -n "$MUTATE" ] && WP255_MUTATE=1 $R cxx-medium-static libprotobuf.a strutil.cc.o m0 time 1
[ -n "$MUTATE" ] && WP255_MUTATE=1 WP255_CLIPS=1 $R cxx-medium-static libprotobuf.a strutil.cc.o m0c time 1
for wl in cxx-medium-static cxx-medium-g-static; do
  W=/Volumes/AgentWork/mortar-helper/.build/bench-work/workloads/$wl
  out=$S/st-$wl; mkdir -p $out
  cwd=$(awk '$1=="cwd"{print $2}' "$W/stamp")
  awk -v o="$out/out" 'prev=="-o"{print o; prev=""; next} /^--dependency-file=/{next} {print; prev=$0}' "$W/response.txt" > "$out/response.txt"
  ( cd "$cwd" && MORTAR_INCR_STATE5="$out/s.state5" "$S/mortar-th" @"$out/response.txt" --incremental=full --incremental-state "$out/s.mortar-state" ) > $out/link.log 2>&1
  ls -la $out >> $out/link.log
  (
    cd /Volumes/AgentWork/mortar-helper/mortar/.worktrees/wp-140 || exit 1
    unset CARGO_TARGET_DIR; source scripts/env.sh
    WP255_STATE=$out/s.state5 WP255_ARCHIVE=$cwd/libprotobuf.a WP255_MEMBER=strutil.cc.o WP255_REPS=9 \
      cargo test --release -p mortar --test wp255_state -- --ignored --nocapture --test-threads=1
  ) > $out/state.log 2>&1
  echo "load $(cut -d' ' -f1 /proc/loadavg)" >> $out/state.log
  cd $out && rm -f out s.state5 s.mortar-state
done
echo "series3 done"
