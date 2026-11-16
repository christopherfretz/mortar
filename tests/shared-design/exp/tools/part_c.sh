#!/bin/bash
# Part C: dlopen/dlsym/dlclose lifecycles, TLS, exceptions, typeinfo,
# -z nodelete, --no-eh-frame-hdr, Fortran.  Usage: part_c.sh WORK
W=$1; S=$(cd "$(dirname "$0")" && pwd); . "$S/common.sh"
C=$W/c; [ -e "$C" ] && { echo "$C exists: use a fresh WORK"; exit 1; }; mkdir -p "$C"; SRC=$S/src/c
L=$C/log.txt; : > "$L"
say() { echo "$*" >> "$L"; }

build() { # arch lk
    local a=$1 k=$2 d=$C/$1/$2; mkdir -p $d
    local CC; CC=$(cc $a c); local X; X=$(cc $a c++); local F; F=$(ldflag $k); local FC; FC=$(cc $a f)
    local IF="-Wl,-init,lib_init -Wl,-fini,lib_fini"
    $CC -O1 -fPIC -shared $F $IF -o $d/libcyc_c.so $SRC/cyc_c.c || say "BUILD FAIL $a $k cyc_c"
    $X -O1 -fPIC -shared $F $IF -o $d/libcyc_cxx.so $SRC/cyc_cxx.cpp || say "BUILD FAIL $a $k cyc_cxx"
    $CC -O1 -fPIC -shared $F -Wl,-z,nodelete -o $d/libnodel.so $SRC/nodel.c || say "BUILD FAIL $a $k nodel"
    $CC -O1 -fPIC -shared $F -o $d/libdel.so $SRC/nodel.c || say "BUILD FAIL $a $k del"
    $X -O1 -fPIC -shared $F -o $d/libtls.so $SRC/tls.cpp || say "BUILD FAIL $a $k tls"
    $X -O1 -fPIC -shared $F -Wl,-soname,libexc.so -o $d/libexc.so $SRC/exc.cpp || say "BUILD FAIL $a $k exc"
    mkdir -p $d/noeh
    $X -O1 -fPIC -shared $F -Wl,--no-eh-frame-hdr -Wl,-soname,libexc.so -o $d/noeh/libexc.so $SRC/exc.cpp \
        || say "BUILD FAIL $a $k exc --no-eh-frame-hdr"
    (cd $d && $FC -O1 -fPIC -shared $F -Wl,-soname,libf.so -o $d/libf.so $SRC/f_lib.f90) || say "BUILD FAIL $a $k fortran lib"
    # hosts: GNU ld, default line
    $CC -O1 -o $d/dlhost $SRC/dlhost.c -ldl || say "BUILD FAIL $a $k dlhost"
    $CC -O1 -o $d/tlshost $SRC/tlshost.c -ldl -lpthread || say "BUILD FAIL $a $k tlshost"
    $X -O1 -I$SRC -o $d/exchost $SRC/exchost.cpp -L$d -lexc || say "BUILD FAIL $a $k exchost"
    (cd $d && $FC -O1 -o $d/fmain $SRC/f_main.f90 -L$d -lf) || say "BUILD FAIL $a $k fmain"
    $CC -O1 -o $d/fcmain $SRC/f_cmain.c -L$d -lf || say "BUILD FAIL $a $k fcmain"
}
for a in $ARCHS; do for k in $LINKERS; do build $a $k; done; done

for a in $ARCHS; do for k in $LINKERS; do
    d=$C/$a/$k; o=$C/$a.$k
    for lib in libcyc_c.so libcyc_cxx.so libnodel.so libdel.so; do
        for m in now-local now-global lazy-local lazy-global; do
            runp $a $d $d/dlhost $d/$lib $m > $o.$lib.$m.out 2>&1; echo "rc=$?" >> $o.$lib.$m.out
        done
    done
    runp $a $d $d/tlshost $d/libtls.so > $o.tls.out 2>&1; echo "rc=$?" >> $o.tls.out
    runp $a $d $d/exchost > $o.exc.out 2>&1; echo "rc=$?" >> $o.exc.out
    : > $o.exc-noeh.out
    for t in 0 1 2 3 4 5 6 7; do
        runp $a $d/noeh $d/exchost $t >> $o.exc-noeh.out 2>&1; echo "t$t rc=$?" >> $o.exc-noeh.out
    done
    runp $a $d $d/fmain > $o.fmain.out 2>&1; echo "rc=$?" >> $o.fmain.out
    runp $a $d $d/fcmain > $o.fcmain.out 2>&1; echo "rc=$?" >> $o.fcmain.out
    {
        echo "## $a $k static facts"
        for lib in libcyc_c.so libcyc_cxx.so libexc.so noeh/libexc.so libnodel.so; do
            echo "-- $lib"
            readelf -lW $d/$lib | grep -E "GNU_EH_FRAME" || echo "   (no PT_GNU_EH_FRAME)"
            readelf -dW $d/$lib | grep -E "\((INIT|FINI|INIT_ARRAY|FINI_ARRAY|INIT_ARRAYSZ|FINI_ARRAYSZ|FLAGS_1|FLAGS)\)"
            readelf -sW $d/$lib | grep -E " __dso_handle$" | sed 's/^/   symtab: /'
            readelf --dyn-syms -W $d/$lib | grep -E "__dso_handle" | sed 's/^/   DYNSYM: /'
        done
    } >> $C/static.txt 2>&1
done; done
python3 -I "$S/check_c.py" "$C" >> "$L"
cat "$L"
