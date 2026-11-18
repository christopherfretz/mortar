#!/bin/bash
# WP-244: format-5 captures of a bench workload's real link (the
# workload's response file, as WP-239's runwl.sh copies it).
#
#   realwl.sh BINDIR WORKLOADS_DIR NAME SCRATCH
#
# BINDIR holds `mortar` (a `testhooks` release build: MORTAR_INCR_STATE5)
# and `incr_state` (mortar_incr's example). Three incremental-mode full
# links: default threads (a), --threads=1 (b), default again (c); each
# writes its format-4 state and, beside it, its format-5 one. Then:
# determinism (canon a = b = c), canon(capture) vs canon(convert(format
# 4)), the size per table of both, and each link's wall and peak RSS.
# Writes SCRATCH/NAME/*.txt; the outputs and states are left for the
# caller to delete by literal path.
set -u
ulimit -c 0
bin=$1; wl=$2/$3; name=$3; out=$4/$3
mkdir -p "$out"
cwd=$(awk '$1=="cwd"{print $2}' "$wl/stamp")
awk -v o="$out/out" 'prev=="-o"{print o; prev=""; next} /^--dependency-file=/{next} {print; prev=$0}' "$wl/response.txt" > "$out/response.txt"
link() { # tag, then extra args
	tag=$1; shift
	( cd "$cwd" && MORTAR_INCR_STATE5="$out/$tag.state5" /usr/bin/time -v "$bin/mortar" @"$out/response.txt" \
		--incremental=full --incremental-state "$out/$tag.mortar-state" "$@" ) > "$out/$tag.log" 2>&1
	echo "$tag rc=$? load=$(cut -d' ' -f1 /proc/loadavg)" >> "$out/links.txt"
	grep -E "Maximum resident|Elapsed" "$out/$tag.log" >> "$out/links.txt"
	grep "^incremental:" "$out/$tag.log" >> "$out/links.txt"
}
: > "$out/links.txt"
link a
link b --threads=1
link c
{
	echo "== a vs b (1 thread)"; "$bin/incr_state" compare "$out/a.state5" "$out/b.state5" | tail -20
	echo "== a vs c"; "$bin/incr_state" compare "$out/a.state5" "$out/c.state5" | tail -20
	echo "== raw bytes"; cmp "$out/a.state5" "$out/b.state5"; cmp "$out/a.state5" "$out/c.state5"
} > "$out/determinism.txt" 2>&1
"$bin/incr_state" convert "$out/a.mortar-state" "$out/a.conv5" 0.0.0 > "$out/convert.txt" 2>&1
"$bin/incr_state" compare "$out/a.conv5" "$out/a.state5" > "$out/canon-diff.txt" 2>&1
{
	echo "== capture"; "$bin/incr_state" size "$out/a.state5"
	echo "== convert(format 4)"; "$bin/incr_state" size "$out/a.mortar-state" 0.0.0
	ls -l "$out/a.mortar-state" "$out/a.state5" "$out/a.conv5"
} > "$out/size.txt" 2>&1
echo "realwl done $name"
