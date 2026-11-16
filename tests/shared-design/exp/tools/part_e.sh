#!/bin/bash
# Part E: mortar as the host's linker (gcc -B DIR with DIR/ld -> mortar),
# linking the hosts of parts A, C and D against each reference's libraries.
# Run after parts a, c, d.  Usage: part_e.sh WORK
W=$1; S=$(cd "$(dirname "$0")" && pwd); . "$S/common.sh"
E=$W/e; [ -e "$E" ] && { echo "$E exists: use a fresh WORK"; exit 1; }; mkdir -p "$E"
L=$E/log.txt; : > "$L"
SA=$S/src/a; SC=$S/src/c; SD=$S/src/d
MB="-B$W/bin/mortar"
# link: out-file, then a driver command; records the exit code and diagnostics
lnk() {
    local out=$1; shift
    "$@" > $out.link.txt 2>&1; local rc=$?
    echo "link-rc=$rc" >> $out.link.txt
    [ $rc -ne 0 ] && { echo "LINK FAIL $out: $(sed "s#$W#W#g" $out.link.txt | tr '\n' ' ' | cut -c1-400)" >> $L; }
    return $rc
}
for a in $ARCHS; do
    CC=$(cc $a c); X=$(cc $a c++); FC=$(cc $a f)
    for k in $LINKERS; do
        e=$E/$a/$k; mkdir -p $e
        da=$W/a/$a/$k; dc=$W/c/$a/$k
        lnk $e/host $CC $MB -g -O0 -o $e/host $SA/host.c -L$da -lfoo -Wl,-rpath-link,$da
        lnk $e/hostcxx $X $MB -g -O0 -o $e/hostcxx $SA/hostcxx.cpp -L$da -lcxx
        lnk $e/dlhost $CC $MB -O1 -o $e/dlhost $SC/dlhost.c -ldl
        lnk $e/tlshost $CC $MB -O1 -o $e/tlshost $SC/tlshost.c -ldl -lpthread
        lnk $e/exchost $X $MB -O1 -I$SC -o $e/exchost $SC/exchost.cpp -L$dc -lexc
        (cd $e && lnk $e/fmain $FC $MB -O1 -I$dc -o $e/fmain $S/src/c/f_main.f90 -L$dc -lf)
        lnk $e/fcmain $CC $MB -O1 -o $e/fcmain $SC/f_cmain.c -L$dc -lf
        for v in def sym; do
            dd=$W/d/$a/$k/$v; mkdir -p $e/d-$v
            lnk $e/d-$v/h1 $CC $MB -O1 -o $e/d-$v/h1 $SD/hd.c -L$dd -lL
            lnk $e/d-$v/h2 $CC $MB -O1 -DDEF -o $e/d-$v/h2 $SD/hd.c -L$dd -lL
            lnk $e/d-$v/h2np $CC $MB -O1 -DDEF -fno-pie -no-pie -o $e/d-$v/h2np $SD/hd.c -L$dd -lL
            lnk $e/d-$v/h6 $CC $MB -O1 -DEXTV -o $e/d-$v/h6 $SD/hd.c -L$dd -lL
            lnk $e/d-$v/h6np $CC $MB -O1 -DEXTV -fno-pie -no-pie -o $e/d-$v/h6np $SD/hd.c -L$dd -lL
            lnk $e/d-$v/h5 $CC $MB -O1 -DDL -DDEF -rdynamic -o $e/d-$v/h5 $SD/hd.c -ldl
        done
    done
done
# run, and compare with the GNU ld host of the same part (same library dir)
cmp_run() { # a k name mortar-prog ref-prog libdir args...
    local a=$1 k=$2 n=$3 mp=$4 rp=$5 d=$6; shift 6
    [ -x $mp ] || { printf '%-4s %-6s %-26s NOT LINKED\n' $a $k "$n"; return; }
    local mo ro
    mo=$(runp $a $d $mp "$@" 2>&1; echo "rc=$?")
    ro=$(runp $a $d $rp "$@" 2>&1; echo "rc=$?")
    echo "$mo" > $E/$a.$k.$n.mortar.out; echo "$ro" > $E/$a.$k.$n.gnuld.out
    # thread output interleaves: tlshost lines are compared sorted, the rest in order
    local srt=cat; [ "$n" = tlshost ] && srt=sort
    if [ "$(echo "$mo" | sed -E 's/[0-9]{5,}/N/g' | $srt)" = "$(echo "$ro" | sed -E 's/[0-9]{5,}/N/g' | $srt)" ]; then r=SAME; else r=DIFFER; fi
    printf '%-4s %-6s %-26s %-6s %s\n' $a $k "$n" $r "$(echo "$mo" | grep -E '^(OK|FAIL|RESULT)|rc=' | tr '\n' ' ' | cut -c1-150)"
}
{
for a in $ARCHS; do for k in $LINKERS; do
    e=$E/$a/$k; da=$W/a/$a/$k; dc=$W/c/$a/$k
    cmp_run $a $k host $e/host $da/host $da
    cmp_run $a $k hostcxx $e/hostcxx $da/hostcxx $da
    for lib in libcyc_c.so libcyc_cxx.so libnodel.so; do for m in now-local lazy-global; do
        cmp_run $a $k "dlhost $lib $m" $e/dlhost $dc/dlhost $dc $dc/$lib $m
    done; done
    cmp_run $a $k tlshost $e/tlshost $dc/tlshost $dc $dc/libtls.so
    cmp_run $a $k exchost $e/exchost $dc/exchost $dc
    cmp_run $a $k fmain $e/fmain $dc/fmain $dc
    cmp_run $a $k fcmain $e/fcmain $dc/fcmain $dc
    for v in def sym; do
        dd=$W/d/$a/$k/$v
        for h in h1 h2 h2np h6 h6np; do cmp_run $a $k "D-$v $h" $e/d-$v/$h $dd/$h $dd; done
        cmp_run $a $k "D-$v h5 local" $e/d-$v/h5 $dd/h5 $dd $dd/libL.so local
        cmp_run $a $k "D-$v h5 deepbind" $e/d-$v/h5 $dd/h5 $dd $dd/libL.so local,deepbind
    done
done; done
} > $E/compare.txt
# target selection: drop the driver's -m and let mortar pick from the objects
for a in $ARCHS; do
    CC=$(cc $a c)
    $CC -c -O1 -o $E/t-$a.o $SA/plug.c
    printf 'int plugfn(int);\nint main(void){return plugfn(1)==3?0:1;}\n' > $E/tm.c
    $CC -c -O1 -o $E/tm-$a.o $E/tm.c
    line=$($CC $MB -v -o $E/tm-$a $E/tm-$a.o $E/t-$a.o 2>&1 | grep -E '/collect2 ' | tail -1)
    args=$(echo "$line" | sed -E 's/^ *[^ ]*collect2 //; s/ -m [a-z0-9_]+//; s/-plugin [^ ]+//; s/-plugin-opt=[^ ]+//g')
    echo "== $a: mortar without -m (driver line minus -m and plugin options)" >> $L
    $MORTAR $args -o $E/tm-$a-nom >> $L 2>&1; echo "rc=$?" >> $L
    [ -x $E/tm-$a-nom ] && readelf -h $E/tm-$a-nom | grep Machine >> $L
    echo "== $a: mortar with the wrong -m" >> $L
    wrong=$([ $a = aa ] && echo elf_x86_64 || echo aarch64linux)
    $MORTAR $args -m $wrong -o $E/tm-$a-wrongm >> $L 2>&1; echo "rc=$?" >> $L
done
echo "== link failures above; run comparison:" >> $L
cat $E/compare.txt >> $L
grep -h -v '^link-rc=0' $(find $E -name '*.link.txt' | sort) 2>/dev/null | sed "s#$W#W#g" | sort | uniq -c | sort -rn | head -30 > $E/diagnostics.txt
echo "== distinct mortar diagnostics (count):" >> $L; cat $E/diagnostics.txt >> $L
cat "$L"
