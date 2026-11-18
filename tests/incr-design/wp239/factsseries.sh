#!/bin/bash
# WP-239 R-4a: the scan's facts (wp239_facts probe) at every hop of a
# series m1series.sh or slotseries.sh made (hN.map, and the changed
# file's copy hN.strutil.o or hN.a.o).
#
#   factsseries.sh PROBE_BIN SERIES_DIR CHANGED(strutil.o|a.o)
#
# Puts each hop's changed file back at its path, runs the probe over the
# hop's own linker words (the map's `arg` records) and writes hN.facts;
# the last hop's file is left in place (as the series left it).
set -u
ulimit -c 0
probe=$1; dir=$2; changed=$3
base=${changed%.o}
n=0
while [ -f "$dir/h$n.map" ]; do
	cp "$dir/h$n.$changed" "$dir/$changed"
	sed -n 's/^arg //p' "$dir/h$n.map" > "$dir/h$n.args"
	WP239_CWD="$dir" WP239_ARGS="$dir/h$n.args" WP239_FACTS_OUT="$dir/h$n.facts" \
		"$probe" --ignored --test-threads=1 > "$dir/h$n.probe.log" 2>&1 || echo "hop $n probe failed"
	n=$((n + 1))
done
echo "factsseries $dir $n hops (base $base)"
