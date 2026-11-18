#!/bin/bash
# WP-244 (WP-239's m1series.sh, and each hop's format-5 state, hN.state5,
# through a `testhooks` mortar's MORTAR_INCR_STATE5): WP-207's M.1 series (review-in72's protobuf stubs, strutil.cc
# edited: base grow1 grow2 rmfn addfn base) as incremental-mode FULL links,
# one per hop, each with its map and state kept, for model.py.
#
#   m1series.sh MORTAR_BINDIR OBJDIR SCRATCH [CXX] [-static extra...]
#
# MORTAR_BINDIR holds `ld` -> mortar (gcc -B). OBJDIR is review-in72's
# m/pb/<cfg> (main.o, the stubs, strutil_<hop>.o). The changed file is
# copied to SCRATCH/strutil.o before each link, so its path is constant.
# Hop rmfn removes safe_strtof's definition while strutil.cc still calls
# it (the reviewer's model never linked it): that hop adds a stub object
# defining it, right after strutil.o, so the series also has a file that
# APPEARS between two stored files (and goes again at addfn).
# CFLAGS for the stub: $STUBFLAGS.
set -u
ulimit -c 0
bin=$1; obj=$2; out=$3; cxx=${4:-g++}
mkdir -p "$out"
others="bytestream common int128 status statusor stringpiece stringprintf structurally_valid substitute"
n=0
for hop in base grow1 grow2 rmfn addfn base; do
	cp "$obj/strutil_$hop.o" "$out/strutil.o"
	objs="$obj/main.o $out/strutil.o"
	if [ "$hop" = rmfn ]; then
		printf 'namespace google { namespace protobuf {\nbool safe_strtof(const char* s, float* v) { *v = 0; return s != 0; }\n} }\n' > "$out/stub.cc"
		$cxx ${STUBFLAGS:--O0 -g} -c "$out/stub.cc" -o "$out/stub.o"
		objs="$objs $out/stub.o"
	fi
	for o in $others; do objs="$objs $obj/$o.o"; done
	( cd "$out" && MORTAR_INCR_STATE5="$out/h$n.state5" $cxx -B"$bin"/ -static -o "$out/out" $objs -Wl,--incremental=full -Wl,--mortar-map="$out/h$n.map" > "$out/h$n.log" 2>&1 )
	echo "hop $n $hop rc=$?"
	cp "$out/out.mortar-state" "$out/h$n.state"
	cp "$out/out" "$out/h$n.out"
	cp "$out/strutil.o" "$out/h$n.strutil.o"
	n=$((n + 1))
done
