#!/bin/bash
# WP-244: a plain (non-incremental) link pays nothing: the workload's
# link by the base's mortar and by this package's (no features),
# alternated, RUNS times each, each run started only while the 1-minute
# load is below 6 (waiting up to an hour in all), wall time and peak RSS
# from /usr/bin/time; then the medians. Then the same for an
# incremental-mode full link (format 4) by this package's plain build,
# and with the format-5 switch (the testhooks build).
#
#   plaincost.sh BINDIR WORKLOADS_DIR NAME SCRATCH [RUNS] [incr]
#
# `incr` as the sixth word: the incremental-mode pair only.
set -u
ulimit -c 0
bin=$1; wl=$2/$3; name=$3; out=$4/$3-cost; runs=${5:-5}; parts=${6:-all}
mkdir -p "$out"
cwd=$(awk '$1=="cwd"{print $2}' "$wl/stamp")
awk -v o="$out/out" 'prev=="-o"{print o; prev=""; next} /^--dependency-file=/{next} {print; prev=$0}' "$wl/response.txt" > "$out/response.txt"
deadline=$(( $(date +%s) + 3600 ))
quiet() {
	while :; do
		l=$(cut -d' ' -f1 /proc/loadavg)
		awk -v l="$l" 'BEGIN{exit !(l<6)}' && return 0
		[ "$(date +%s)" -gt "$deadline" ] && return 1
		sleep 15
	done
}
one() { # tag binary extra...
	tag=$1; b=$2; shift 2
	quiet || echo "LOAD NOT BELOW 6" >> "$out/runs.tsv"
	l=$(cut -d' ' -f1 /proc/loadavg)
	( cd "$cwd" && /usr/bin/time -f "%e %M" -o "$out/t.txt" "$b" @"$out/response.txt" "$@" ) > "$out/$tag.log" 2>&1
	printf '%s\t%s\t%s\n' "$tag" "$(cat "$out/t.txt")" "$l" >> "$out/runs.tsv"
}
: > "$out/runs.tsv"
[ "$parts" = incr ] || for i in $(seq "$runs"); do
	one base "$bin/mortar-base"
	one plain "$bin/mortar-plain"
done
for i in $(seq "$runs"); do
	one incr4 "$bin/mortar-plain" --incremental=full --incremental-state "$out/s4"
	MORTAR_INCR_STATE5="$out/s5" one incr5 "$bin/mortar" --incremental=full --incremental-state "$out/s5.mortar-state"
done
python3 - "$out/runs.tsv" <<'P' > "$out/medians.txt"
import sys, statistics as st
rows = {}
for l in open(sys.argv[1]):
    p = l.split()
    if len(p) == 4:
        rows.setdefault(p[0], []).append((float(p[1]), int(p[2]), float(p[3])))
for k, v in rows.items():
    print(f"{k}: wall median {st.median(x[0] for x in v):.3f} s, peak RSS median {st.median(x[1] for x in v)/1024:.1f} MiB, loads {[x[2] for x in v]}")
P
cat "$out/medians.txt"
echo "plaincost done $name"
