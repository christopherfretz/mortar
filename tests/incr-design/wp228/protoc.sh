#!/bin/bash
# protoc.sh SCRATCH: item 5 on protoc -g (the bench workload
# cxx-medium-g-static, inputs read-only): an incremental-mode full link by
# mortar with its map, then measure.py over it. The output and state are
# left for the caller (delete by literal path).
set -u
ulimit -c 0
HERE="$(cd "$(dirname "$0")" && pwd)"
S="$1"
WL=/Volumes/AgentWork/mortar-helper/.build/bench-work/workloads/cxx-medium-g-static
M=/Volumes/AgentWork/mortar-helper/.build/target-wp-154/release/mortar
while [ "$(cut -d' ' -f1 /proc/loadavg | cut -d. -f1)" -ge 12 ]; do sleep 20; done
python3 -I - "$WL/response.txt" "$S/protoc.rsp" "$S/protoc" <<'PY'
import sys
src, dst, out = sys.argv[1:]
w = open(src).read().split('\n')
r = []
skip = False
for x in w:
    if skip:
        skip = False; r.append(out); continue
    if x == '-o':
        skip = True; r.append(x); continue
    if x.startswith('-plugin') or x.startswith('--dependency-file'):
        continue
    r.append(x)
# drop the plugin's path that followed -plugin
r = [x for x in r if not x.endswith('liblto_plugin.so')]
open(dst, 'w').write('\n'.join(r))
PY
cd "$WL/build" || exit 1
for mode in plain incr; do
  case $mode in
    plain) extra="";;
    incr) extra="--incremental --incremental-state=$S/protoc.state --mortar-map=$S/protoc.map";;
  esac
  # shellcheck disable=SC2086
  /usr/bin/time -f "$mode wall %e s maxrss %M KB" "$M" @"$S/protoc.rsp" $extra 2>> "$S/protoc.time"
  echo "$mode rc=$?" >> "$S/protoc.time"
  ls -l "$S/protoc" >> "$S/protoc.time"
done
python3 -I "$HERE/measure.py" --map "$S/protoc.map" --out "$S/protoc" > "$S/protoc.measure" 2>&1
echo "measure rc=$?" >> "$S/protoc.time"
echo "PROTOC-DONE"
