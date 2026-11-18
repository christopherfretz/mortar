#!/bin/bash
# protoc_rows.sh PROBE OUT WORKLOAD -- WP-238 review, item 4/6: one probe
# link (rows dumped) of a bench workload's captured link plus
# --gc-sections, as the author's plaincost.sh runs it (response file from
# the author's scratch, READ ONLY; bench-work READ ONLY), then rowstats.py
# over the dump; the dump and the output are deleted. Last line FINAL.
ulimit -c 0
PROBE=$1; OUT=$2; w=$3
here=$(cd "$(dirname "$0")" && pwd)
resp=/Volumes/AgentWork/mortar-helper/.build/target-wp-125/wp238/resp/$w.txt
cwd=$(grep '^cwd' "/Volumes/AgentWork/mortar-helper/.build/bench-work/workloads/$w/stamp" | cut -f2)
mkdir -p "$OUT/dump"
while [ "$(cut -d' ' -f1 /proc/loadavg | cut -d. -f1)" -ge 12 ]; do sleep 15; done
(cd "$cwd" && MORTAR_WP238=$OUT/dump MORTAR_WP238_ROWS=1 "$PROBE" @"$resp" --gc-sections -o "$OUT/out.bin" 2> "$OUT/link.err")
echo "link rc $?"
grep -h '^replay\|^stored-form\|^total' "$OUT"/dump/*.sum
python3 -I "$here/rowstats.py" "$(ls "$OUT"/dump/*.rows | head -1)"
rm -f "$OUT/out.bin"
echo "FINAL protoc_rows"
