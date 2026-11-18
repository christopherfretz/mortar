#!/bin/bash
# WP-244 (WP-239's slotseries.sh, and each hop's format-5 state, hN.state5,
# through a `testhooks` mortar's MORTAR_INCR_STATE5): a series whose edits add and retire GOT, IPLT (ifunc) and
# TLS initial-exec slots, as incremental-mode FULL links, one per hop
# (hN.map, hN.out, hN.state, hN.a.o), for model.py.
#
#   slotseries.sh MORTAR_BINDIR SCRATCH CC
#
# a.c is the changed file; b.c also uses gx (a slot two files demand);
# c.c defines two ifuncs; d.c defines the TLS variables. Hops: v0 v1 v2 v3
# v4 v0 (v1 drops gx: kept, b still uses it; v2 drops gy, adds gz: one
# retired, one new; v3 calls f2 instead of f1: IPLT retired and appended;
# v4 drops tx: the TLS IE slot retired; v0 again: everything back).
set -eu
ulimit -c 0
bin=$1; out=$2; cc=$3
mkdir -p "$out"
cd "$out"
cat > b.c <<'X'
extern int gx;
int b_use(void) { return gx + 1; }
X
cat > c.c <<'X'
static int f1_impl(void) { return 1; }
static int f2_impl(void) { return 2; }
static void *f1_resolve(void) { return (void *)f1_impl; }
static void *f2_resolve(void) { return (void *)f2_impl; }
int f1(void) __attribute__((ifunc("f1_resolve")));
int f2(void) __attribute__((ifunc("f2_resolve")));
int gx = 3, gy = 4, gz = 5;
X
cat > d.c <<'X'
__thread int tx = 7;
__thread int ty = 8;
int d_use(void) { return ty; }
X
cat > main.c <<'X'
int a_use(void); int b_use(void); int d_use(void);
int main(void) { return (a_use() + b_use() + d_use()) == 0; }
X
mk() { # name, then the body of a_use
	printf 'extern int gx, gy, gz; int f1(void); int f2(void);\nextern __thread int tx;\nint a_use(void) { return %s; }\n' "$2" > "a_$1.c"
}
mk v0 'gx + gy + f1() + tx'
mk v1 'gy + f1() + tx'
mk v2 'gz + f1() + tx'
mk v3 'gz + f2() + tx'
mk v4 'gz + f2()'
fl="-O2 -fPIC -ftls-model=initial-exec"
for f in b c d main a_v0 a_v1 a_v2 a_v3 a_v4; do $cc $fl -c $f.c -o $f.o; done
n=0
for v in v0 v1 v2 v3 v4 v0; do
	cp "a_$v.o" a.o
	MORTAR_INCR_STATE5="$out/h$n.state5" $cc -B"$bin"/ -static -o "$out/out" main.o a.o b.o c.o d.o -Wl,--incremental=full -Wl,--mortar-map="$out/h$n.map" > "h$n.log" 2>&1 || echo "hop $n link failed"
	cp out.mortar-state "h$n.state"; cp out "h$n.out"; cp a.o "h$n.a.o"
	n=$((n + 1))
done
echo "slotseries done"
