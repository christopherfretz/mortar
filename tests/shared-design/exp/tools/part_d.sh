#!/bin/bash
# Part D: how glibc resolves a library's references to its own
# default-visibility f and v, per host case, per linker, per L variant
# (default, -Bsymbolic, -Bsymbolic-functions).  Usage: part_d.sh WORK
W=$1; S=$(cd "$(dirname "$0")" && pwd); . "$S/common.sh"
D=$W/d; [ -e "$D" ] && { echo "$D exists: use a fresh WORK"; exit 1; }; mkdir -p "$D"; SRC=$S/src/d
L=$D/log.txt; : > "$L"
say() { echo "$*" >> "$L"; }
VARIANTS="def sym symf"
vflag() { case $1 in def) echo "" ;; sym) echo "-Wl,-Bsymbolic" ;; symf) echo "-Wl,-Bsymbolic-functions" ;; esac; }

for a in $ARCHS; do for k in $LINKERS; do
    CC=$(cc $a c); F=$(ldflag $k)
    for v in $VARIANTS; do
        d=$D/$a/$k/$v; mkdir -p $d
        $CC -O1 -fPIC -shared $F $(vflag $v) -Wl,-soname,libL.so -o $d/libL.so $SRC/L.c || say "BUILD FAIL $a $k $v L"
        $CC -O1 -fPIC -shared $F -Wl,-soname,libM.so -o $d/libM.so $SRC/M.c || say "BUILD FAIL $a $k M"
        $CC -O1 -fPIC -shared $F -Wl,-soname,libP.so -o $d/libP.so $SRC/P.c || say "BUILD FAIL $a $k P"
        # hosts: GNU ld, default line
        $CC -O1 -o $d/h1 $SRC/hd.c -L$d -lL || say "BUILD FAIL $a $k $v h1"
        $CC -O1 -DDEF -o $d/h2 $SRC/hd.c -L$d -lL || say "BUILD FAIL $a $k $v h2"
        $CC -O1 -DDEF -fno-pie -no-pie -o $d/h2np $SRC/hd.c -L$d -lL || say "BUILD FAIL $a $k $v h2np"
        $CC -O1 -DEXTV -o $d/h6 $SRC/hd.c -L$d -lL || say "BUILD FAIL $a $k $v h6"
        $CC -O1 -DEXTV -fno-pie -no-pie -o $d/h6np $SRC/hd.c -L$d -lL || say "BUILD FAIL $a $k $v h6np"
        # case 6 also with the host linked by the same linker as L (does any diagnose
        # a copy relocation against a -Bsymbolic library?)
        $CC -O1 -DEXTV $F -o $d/h6k $SRC/hd.c -L$d -lL > $d/h6k.link.txt 2>&1 || say "BUILD FAIL $a $k $v h6k"
        $CC -O1 -DEXTV -fno-pie -no-pie $F -o $d/h6knp $SRC/hd.c -L$d -lL > $d/h6knp.link.txt 2>&1 || say "BUILD FAIL $a $k $v h6knp"
        $CC -O1 -DDL -o $d/h3 $SRC/hd.c -ldl || say "BUILD FAIL $a $k $v h3"
        $CC -O1 -DDL -o $d/h3n $SRC/hd.c -ldl -L$d -Wl,--no-as-needed -lM || say "BUILD FAIL $a $k $v h3n"
        $CC -O1 -DDL -DDEF -rdynamic -o $d/h5 $SRC/hd.c -ldl || say "BUILD FAIL $a $k $v h5"
        {
            echo "## $a $k $v libL.so dynamic relocations and flags"
            readelf -rW $d/libL.so | grep -E " (f|v)( \+ [0-9a-f]+)?$" | awk '{print "   ", $3, $5}'
            echo "    RELATIVE: $(readelf -rW $d/libL.so | grep -c RELATIVE)"
            readelf -dW $d/libL.so | grep -E "\((FLAGS|FLAGS_1|SYMBOLIC)\)" | sed 's/^/   /'
            readelf --dyn-syms -W $d/libL.so | awk '$8=="f"||$8=="v"{print "    dynsym", $8, $4, $5, $6, $7}'
        } >> $D/relocs.txt
    done
done; done

run_case() { # a d label env|"" prog args...
    local a=$1 d=$2 lab=$3 e=$4; shift 4
    local out
    if [ -n "$e" ]; then out=$(runpe $a $d "$e" "$@" 2>&1)
    else out=$(runp $a $d "$@" 2>&1); fi
    printf '%-44s %s\n' "$lab" "$(echo "$out" | tr '\n' ' ')"
}
for a in $ARCHS; do for k in $LINKERS; do for v in $VARIANTS; do
    d=$D/$a/$k/$v
    {
        echo "## $a $k L=$v"
        run_case $a $d "1 exe does not define f/v" "" $d/h1
        run_case $a $d "2 exe defines f,v (PIE)" "" $d/h2
        run_case $a $d "2 exe defines f,v (-no-pie)" "" $d/h2np
        run_case $a $d "3 dlopen L local (nothing else)" "" $d/h3 $d/libL.so local
        run_case $a $d "3 dlopen M global, then L local" "" $d/h3 $d/libL.so local $d/libM.so global
        run_case $a $d "3 dlopen M local, then L local" "" $d/h3 $d/libL.so local $d/libM.so local
        run_case $a $d "3 M DT_NEEDED of exe, dlopen L local" "" $d/h3n $d/libL.so local
        run_case $a $d "4 LD_PRELOAD P, exe does not define" "LD_PRELOAD=$d/libP.so" $d/h1
        run_case $a $d "4 LD_PRELOAD P, exe defines f,v" "LD_PRELOAD=$d/libP.so" $d/h2
        run_case $a $d "5 exe -rdynamic defines f,v; L local" "" $d/h5 $d/libL.so local
        run_case $a $d "5 same, L local|DEEPBIND" "" $d/h5 $d/libL.so local,deepbind
        run_case $a $d "5 DEEPBIND L after M global" "" $d/h3 $d/libL.so local,deepbind $d/libM.so global
        run_case $a $d "6 exe writes extern v=99 (PIE)" "" $d/h6
        run_case $a $d "6 exe writes extern v=99 (-no-pie)" "" $d/h6np
        run_case $a $d "6 same, host linked by L's linker (PIE)" "" $d/h6k
        run_case $a $d "6 same, host by L's linker (-no-pie)" "" $d/h6knp
    } >> $D/results.txt
done; done; done
for a in $ARCHS; do for k in $LINKERS; do for v in $VARIANTS; do for h in h6k h6knp; do
    f=$D/$a/$k/$v/$h.link.txt; [ -s $f ] && { echo "== $a $k $v $h link diagnostics:"; sed "s#$W#W#g" $f; } >> $D/h6k-diagnostics.txt
done; done; done; done
for a in $ARCHS; do echo "== $a h6 (PIE) and h6np (-fno-pie -no-pie) relocations against v (GNU ld host, L=sym of bfd)"
    readelf -rW $D/$a/bfd/sym/h6 | grep -E " v \+"; readelf -rW $D/$a/bfd/sym/h6np | grep -E " v \+"; done >> $D/relocs.txt
python3 -I "$S/check_d.py" "$D" >> "$L"
echo "== case 6 link diagnostics (hosts linked by each linker):" >> "$L"
cat $D/h6k-diagnostics.txt >> "$L" 2>/dev/null || echo "   (none: every link was silent)" >> "$L"
cat "$L"
