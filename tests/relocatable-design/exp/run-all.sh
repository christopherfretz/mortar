#!/bin/bash
# run-all.sh WORK: every experiment of WP-154 into a fresh directory WORK, one
# subdirectory and one log per area. Serial (one link at a time); BusyBox's
# build is make -j4. About 25 minutes on the aarch64 host; WORK needs ~1.2 GB
# while bench/ runs (its three 250-300 MB outputs: delete them afterwards).
# Paths: mortar is /Volumes/AgentWork/mortar-helper/.build/target-wp-154/mortar-3ad9867a
# (final/, busybox/, debug/, none-overlap/, meta/, more/ use it), kiln++ the
# read-only .build/target-kiln-next build (meta/), the cxx-large-g build tree of
# .build/bench-work, read-only (bench/).
set -u; ulimit -c 0
W=$(realpath -m "$1"); H=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$W"
run() { local a=$1; shift; echo "== $a"; "$@" > "$W/$a.log" 2>&1; echo "   rc=$? $(tail -1 "$W/$a.log")"; }
run census       "$H/census/run.sh" "$W/census"
run busybox      "$H/busybox/run.sh" "$W/busybox"
run final        "$H/final/run.sh" "$W/final"
run facts        "$H/facts/run.sh" "$W/facts"
run facts-supp   "$H/facts/supp.sh" "$W/facts"
run options      "$H/options/run.sh" "$W/options"
run moldbug      "$H/moldbug/run.sh" "$W/moldbug"
run debug        "$H/debug/run.sh" "$W/debug"
run none-overlap "$H/none-overlap/run.sh" "$W/none-overlap"
run meta         "$H/meta/run.sh" "$W/meta"
run more         "$H/more/run.sh" "$W/more"
run prelink      "$H/more/prelink.sh" "$W/prelink"
run bench        "$H/bench/run.sh" "$W/bench" 400
echo "run-all done"
