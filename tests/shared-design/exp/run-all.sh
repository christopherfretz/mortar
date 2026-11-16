#!/bin/bash
# run-all.sh WORK: re-run every WP-110 (shared-object output design) experiment
# into a fresh directory WORK. Each area writes WORK/<area>; its log is
# WORK/<area>.log. Areas run serially (the host is shared); about 5 min.
# Needs the mortar binary built at the branch point (dd7fe7d):
# /Volumes/AgentWork/mortar-helper/.build/target-wp-110/release/mortar
# (census and tools run it; every -shared line is refused by it).
set -u
[ $# -eq 1 ] || { echo "usage: run-all.sh WORK" >&2; exit 2; }
ulimit -c 0
WORK=$(realpath -m "$1"); mkdir -p "$WORK"; cd "$WORK" || exit 1
HERE=/Volumes/AgentWork/mortar-helper/.build/target-wp-110/exp
rc=0
for area in census export dyn tools; do
  [ -f "$HERE/$area/run.sh" ] || { echo "== $area: no run.sh"; rc=1; continue; }
  echo "== $area"   # each run.sh creates WORK/<area> itself (it refuses an existing one)
  ( cd "$WORK" && bash "$HERE/$area/run.sh" "$WORK/$area" ) > "$WORK/$area.log" 2>&1 \
    || { echo "   $area failed (see $WORK/$area.log)"; rc=1; }
done
exit $rc
