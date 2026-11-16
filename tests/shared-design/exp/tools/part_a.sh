#!/bin/bash
# Part A: debuggers on shared objects each reference made.
# Usage: part_a.sh WORK   (WORK already holds bin/; logs in WORK/a)
W=$1; S=$(cd "$(dirname "$0")" && pwd); . "$S/common.sh"
A=$W/a; [ -e "$A" ] && { echo "$A exists: use a fresh WORK"; exit 1; }; mkdir -p "$A"; SRC=$S/src/a
L=$A/log.txt; : > "$L"
line() { grep -n "$1" "$2" | head -1 | cut -d: -f1; }
HOSTLINE=$(line HOST_CALL_LINE $SRC/host.c)
CXXLINE=$(line CXX_GET_LINE $SRC/cxx.cpp)

build() { # arch lk
    local a=$1 k=$2 d=$A/$1/$2; mkdir -p $d
    local C; C=$(cc $a c); local X; X=$(cc $a c++); local F; F=$(ldflag $k)
    $C -g -O0 -fPIC -shared $F -Wl,-soname,libbar.so -o $d/libbar.so $SRC/bar.c &&
    $C -g -O0 -fPIC -shared $F -Wl,-soname,libfoo.so -o $d/libfoo.so $SRC/foo.c -L$d -lbar &&
    $C -g -O0 -fPIC -shared $F -Wl,-soname,libplug.so -o $d/libplug.so $SRC/plug.c &&
    $X -g -O0 -fPIC -shared $F -Wl,-soname,libcxx.so -o $d/libcxx.so $SRC/cxx.cpp &&
    # hosts: GNU ld, default driver line (PIE)
    $C -g -O0 -o $d/host $SRC/host.c -L$d -lfoo -Wl,-rpath-link,$d &&
    $X -g -O0 -o $d/hostcxx $SRC/hostcxx.cpp -L$d -lcxx || echo "BUILD FAIL $a $k" >> $L
}
for a in $ARCHS; do for k in $LINKERS; do build $a $k; done; done

# ---- gdb, native aarch64 ------------------------------------------------
gdb_session() { # d
    local d=$1
    LD_LIBRARY_PATH=$d timeout 120 gdb -nx -batch \
      -ex 'set pagination off' -ex 'set breakpoint pending on' \
      -ex 'set debuginfod enabled off' \
      -ex 'break libfn' -ex 'break bar_fn' -ex 'break plugfn' \
      -ex 'run' \
      -ex 'echo ==STOP1 libfn\n' -ex 'info sharedlibrary' -ex 'bt' \
      -ex 'print foo_global' -ex 'print foo_tls' -ex 'info line *$pc' \
      -ex 'continue' \
      -ex 'echo ==STOP2 bar_fn\n' -ex 'bt' -ex 'finish' \
      -ex 'echo ==AFTER-FINISH\n' -ex 'info line *$pc' \
      -ex 'continue' \
      -ex 'echo ==STOP3 plugfn (pending before dlopen)\n' -ex 'bt' -ex 'print plug_global' \
      -ex 'info sharedlibrary' -ex 'continue' $d/host 2>&1
}
gdb_after_dlopen() { # breakpoint set after the dlopen
    local d=$1
    LD_LIBRARY_PATH=$d timeout 120 gdb -nx -batch \
      -ex 'set pagination off' -ex 'set debuginfod enabled off' \
      -ex 'break after_dlopen' -ex 'run' \
      -ex 'echo ==AT after_dlopen\n' -ex 'break plugfn' -ex 'continue' \
      -ex 'echo ==STOP plugfn (set after dlopen)\n' -ex 'bt 1' -ex 'continue' $d/host 2>&1
}
gdb_step() { # step from the executable through the PLT into libfn
    local d=$1
    LD_LIBRARY_PATH=$d timeout 120 gdb -nx -batch \
      -ex 'set pagination off' -ex 'set debuginfod enabled off' \
      -ex "break host.c:$HOSTLINE" -ex 'run' -ex 'echo ==STEP\n' -ex 'step' \
      -ex 'bt 1' -ex 'step' -ex 'step' -ex 'echo ==STEP into bar_fn\n' -ex 'bt 2' \
      -ex 'kill' $d/host 2>&1
}
gdb_cxx() {
    local d=$1
    LD_LIBRARY_PATH=$d timeout 120 gdb -nx -batch \
      -ex 'set pagination off' -ex 'set breakpoint pending on' -ex 'set debuginfod enabled off' \
      -ex 'break ns::Widget::get' -ex 'run' -ex 'echo ==CXX\n' -ex 'bt' \
      -ex 'print *this' -ex 'print ns::cxx_tls' -ex 'print ns::cxx_name' \
      -ex 'continue' -ex 'echo ==CXX2\n' -ex 'print *this' -ex 'continue' $d/hostcxx 2>&1
}
gdb_cxx_line() { # a line breakpoint in the library (no PLT location)
    local d=$1
    LD_LIBRARY_PATH=$d timeout 120 gdb -nx -batch \
      -ex 'set pagination off' -ex 'set breakpoint pending on' -ex 'set debuginfod enabled off' \
      -ex "break cxx.cpp:$CXXLINE" -ex 'run' -ex 'echo ==CXXLINE\n' -ex 'bt' \
      -ex 'print *this' -ex 'print ns::cxx_tls' -ex 'finish' -ex 'continue' $d/hostcxx 2>&1
}
lldb_session() {
    local d=$1
    LD_LIBRARY_PATH=$d timeout 120 lldb-19 -b -x \
      -o 'settings set target.disable-aslr false' \
      -o 'settings set target.env-vars LD_LIBRARY_PATH='"$d" \
      -o 'breakpoint set -n libfn' -o 'breakpoint set -n bar_fn' -o 'breakpoint set -n plugfn' \
      -o 'run' -o 'script print("==STOP1 libfn")' \
      -o 'image list -b' -o 'bt' -o 'p foo_global' \
      -o 'continue' -o 'script print("==STOP2 bar_fn")' -o 'bt' \
      -o 'finish' -o 'script print("==AFTER-FINISH")' -o 'frame info' \
      -o 'continue' -o 'script print("==STOP3 plugfn")' -o 'bt' -o 'p plug_global' \
      -o 'image list -b' -o 'continue' $d/host 2>&1
}
lldb_tls() { # lldb stops a batch at the first failing command: TLS reads go last
    local d=$1
    LD_LIBRARY_PATH=$d timeout 120 lldb-19 -b -x \
      -o 'settings set target.disable-aslr false' \
      -o 'settings set target.env-vars LD_LIBRARY_PATH='"$d" \
      -o 'breakpoint set -n libfn' -o run -o 'script print("==TLS")' \
      -o 'target variable foo_tls' -o 'p foo_tls' $d/host 2>&1
}
lldb_tls_cxx() {
    local d=$1
    LD_LIBRARY_PATH=$d timeout 120 lldb-19 -b -x \
      -o 'settings set target.disable-aslr false' \
      -o 'settings set target.env-vars LD_LIBRARY_PATH='"$d" \
      -o "breakpoint set -f cxx.cpp -l $CXXLINE" -o run -o 'script print("==TLS")' -o 'p *this' \
      -o 'target variable ns::cxx_tls' -o 'p ns::cxx_tls' $d/hostcxx 2>&1
}
lldb_step() {
    local d=$1
    LD_LIBRARY_PATH=$d timeout 120 lldb-19 -b -x \
      -o 'settings set target.disable-aslr false' \
      -o 'settings set target.env-vars LD_LIBRARY_PATH='"$d" \
      -o "breakpoint set -f host.c -l $HOSTLINE" -o run -o 'script print("==STEP")' \
      -o 'thread step-in' -o 'frame info' -o 'kill' $d/host 2>&1
}
lldb_cxx() {
    local d=$1
    LD_LIBRARY_PATH=$d timeout 120 lldb-19 -b -x \
      -o 'settings set target.disable-aslr false' \
      -o 'settings set target.env-vars LD_LIBRARY_PATH='"$d" \
      -o 'breakpoint set -n ns::Widget::get' -o run -o 'script print("==CXX")' -o bt \
      -o 'p *this' -o 'p ns::cxx_name' -o continue $d/hostcxx 2>&1
}
for k in $LINKERS; do
    d=$A/aa/$k
    for t in gdb_session gdb_after_dlopen gdb_step gdb_cxx gdb_cxx_line lldb_session lldb_step lldb_cxx lldb_tls lldb_tls_cxx; do
        $t $d > $A/aa.$k.$t.out
        norm < $A/aa.$k.$t.out > $A/aa.$k.$t.norm
    done
done
# ---- gdb-multiarch against qemu-x86_64 -g --------------------------------
freeport() { python3 -I -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()'; }
qgdb() { # d prog gdb-ex...
    local d=$1 p=$2; shift 2
    local port; port=$(freeport)
    qemu-x86_64 -L $X64ROOT -E LD_LIBRARY_PATH=$d -g $port $d/$p > $A/qemu.stdout 2>&1 &
    local qp=$!
    sleep 1
    local args=()
    for e in "$@"; do args+=(-ex "$e"); done
    timeout 120 gdb-multiarch -nx -batch -ex 'set pagination off' -ex 'set debuginfod enabled off' \
        -ex "set sysroot $X64ROOT" -ex 'set breakpoint pending on' -ex "set solib-search-path $d" \
        -ex "file $d/$p" -ex "target remote 127.0.0.1:$port" "${args[@]}" 2>&1
    kill $qp 2>/dev/null; wait $qp 2>/dev/null
    echo "--- program stdout:"; cat $A/qemu.stdout
}
for k in $LINKERS; do
    d=$A/x64/$k
    qgdb $d host 'break libfn' 'break bar_fn' 'break plugfn' 'continue' \
        'echo ==STOP1 libfn\n' 'info sharedlibrary' 'bt' 'print foo_global' 'print foo_tls' \
        'continue' 'echo ==STOP2 bar_fn\n' 'bt' 'finish' \
        'continue' 'echo ==STOP3 plugfn\n' 'bt' 'print plug_global' 'info sharedlibrary' \
        'continue' > $A/x64.$k.qgdb.out
    qgdb $d host "break host.c:$HOSTLINE" 'continue' 'echo ==STEP\n' 'step' 'bt 1' \
        'continue' > $A/x64.$k.qgdb_step.out
    qgdb $d hostcxx "break cxx.cpp:$CXXLINE" 'continue' 'echo ==CXX\n' 'bt' 'print *this' \
        'print ns::cxx_tls' 'continue' > $A/x64.$k.qgdb_cxx.out
    qgdb $d hostcxx 'break ns::Widget::get' 'continue' 'echo ==CXXNAME\n' 'bt 1' 'info breakpoints' \
        'continue' > $A/x64.$k.qgdb_cxxname.out
    for t in qgdb qgdb_step qgdb_cxx qgdb_cxxname; do norm < $A/x64.$k.$t.out > $A/x64.$k.$t.norm; done
done
# ---- comparisons ---------------------------------------------------------
for f in $(cd $A && ls aa.bfd.*.norm x64.bfd.*.norm); do
    t=${f#*.bfd.}; ar=${f%%.*}
    for k in lld22 lld19 mold; do
        if diff -q $A/$f $A/$ar.$k.$t > /dev/null; then echo "SAME   $ar $k vs bfd: $t" >> $L
        else echo "DIFFER $ar $k vs bfd: $t" >> $L; diff $A/$f $A/$ar.$k.$t | head -40 >> $L; fi
    done
done
python3 -I "$S/check_a.py" "$A" >> "$L"
cat "$L"
