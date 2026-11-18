#!/bin/bash
# WP-239 R-4a: a control series for facts.py, where the scan's facts of
# keys that an unchanged file references DO change (one hop each).
#
#   ctlseries.sh MORTAR_BINDIR SCRATCH CC
#
# a.c is the changed file; b.c (unchanged) calls f, h and al; c.c defines
# h; `--defsym=al=f` on every link. Hops: v0; v1 makes f an ifunc (f's
# kind and type change: (slots) and symbol-kind-changed both); v0 again;
# v2 adds a `.gnu.warning.h` section (a.c does not mention h: h's
# gnu_warning flips, h is no affected key); v0 again. Writes hN.map,
# hN.a.o like slotseries.sh.
set -eu
ulimit -c 0
bin=$1; out=$2; cc=$3
mkdir -p "$out"
cd "$out"
printf 'int f(void); int h(void); int al(void);\nint b_use(void) { return f() + h() + al(); }\n' > b.c
printf 'int h(void) { return 3; }\n' > c.c
printf 'int b_use(void);\nint main(void) { return b_use() == 0; }\n' > main.c
printf 'int f(void) { return 1; }\n' > a_v0.c
printf 'static int f_impl(void) { return 1; }\nstatic void *f_resolve(void) { return (void *)f_impl; }\nint f(void) __attribute__((ifunc("f_resolve")));\n' > a_v1.c
printf 'int f(void) { return 1; }\nstatic const char w[] __attribute__((section(".gnu.warning.h"), used)) = "h is bad";\n' > a_v2.c
for f in b c main a_v0 a_v1 a_v2; do $cc -O2 -c $f.c -o $f.o; done
n=0
for v in v0 v1 v0 v2 v0; do
	cp "a_$v.o" a.o
	$cc -B"$bin"/ -static -o "$out/out" main.o a.o b.o c.o -Wl,--defsym=al=f -Wl,--incremental=full -Wl,--mortar-map="$out/h$n.map" > "h$n.log" 2>&1 || echo "hop $n link failed"
	cp a.o "h$n.a.o"
	n=$((n + 1))
done
echo "ctlseries done"
