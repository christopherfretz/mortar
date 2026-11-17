#!/bin/bash
# WP-077: the "fail the load" and audit experiments (native glibc only).
# usage: run-exp.sh WORK MATRIX_WORK   (MATRIX_WORK: a run.sh work directory with h1.so ... h2lazy.so)
set -u; ulimit -c 0
W=${1:?}; M=${2:?}; E=$(cd "$(dirname "$0")" && pwd); mkdir -p "$W"; cd "$W" || exit 1
gcc -O2 -fPIC -shared "$E/audit.c" -o audit.so
gcc -O2 -fPIC -shared "$E/refuse.c" -o refuse.so
gcc -O2 "$E/fl-host.c" -o fl-host -ldl
gcc -O2 -fPIC -shared "$E/failload.c" -o failload-close.so -ldl
ldso=$(gcc -print-file-name=ld-linux-aarch64.so.1); [ -f "$ldso" ] || ldso=/lib/ld-linux-aarch64.so.1
gcc -O2 -fPIC -shared -DVIA_SIGNAL "$E/failload.c" -o failload-signal.so -ldl "$ldso" && echo "linked against $ldso"
echo "== FL1 constructor calls _dl_signal_error (GLIBC_PRIVATE)"
./fl-host ./failload-signal.so; echo "    exit=$?"
echo "== FL2 constructor dlcloses its own handle"
./fl-host ./failload-close.so; echo "    exit=$?"
echo "== FL3 audit la_objsearch refuses h2.so before it is mapped"
LD_AUDIT=./refuse.so ./fl-host "$M/h2.so"; echo "    exit=$?"
echo "== AUD-1 la_symbind: R1m's opens (-z now modules), h2 LOCAL, h1 GLOBAL, h1b LOCAL"
LD_AUDIT=./audit.so "$M/host" o:"$M/h2.so": o:"$M/h1.so":g o:"$M/h1b.so": c:"$M/h1b.so" 2>&1 | grep -v objopen
echo "== AUD-2 la_symbind: R3m's opens (lazy modules), then the call that binds"
LD_AUDIT=./audit.so "$M/host" o:"$M/h2lazy.so":l o:"$M/h1lazy.so":gl c:"$M/h2lazy.so" 2>&1 | grep -v objopen
for v in 1 2; do gcc -O1 -fPIC -shared -DDV=$v -Dplugin_entry=plugin_entry "$E/dvar.c" -o dv$v.so; done
echo "== AUD-3 la_symbind for a data reference (GLOB_DAT): dv1 GLOBAL, dv2 LOCAL (binds dv1's dvar)"
cat > dvhost.c <<'C'
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
int main(int c, char** v) { void* a = dlopen(v[1], RTLD_NOW | RTLD_GLOBAL); void* b = dlopen(v[2], RTLD_NOW);
  int (*f)(void) = (int (*)(void))dlsym(b, "dvar_entry"); printf("dv2's dvar_entry -> %d\n", f()); return a == 0; }
C
gcc -O2 dvhost.c -o dvhost -ldl
LD_AUDIT=./audit.so ./dvhost ./dv1.so ./dv2.so 2>&1 | grep -v objopen
echo "== AUD-4 cost: a C++ program's start (clang-19 --version: libLLVM, libclang-cpp) with and without LD_AUDIT, and with LD_BIND_NOW (the -z now price)"
for k in 1 2 3 4 5; do
  for mode in plain bindnow audit; do
    s=$(date +%s%N)
    case $mode in plain) clang-19 --version > /dev/null ;; bindnow) LD_BIND_NOW=1 clang-19 --version > /dev/null ;; audit) LD_AUDIT=./audit.so clang-19 --version > /dev/null 2>&1 ;; esac
    echo "    $mode $(( ($(date +%s%N) - s) / 1000 )) us"
  done
done | sort | awk '{a[$1]=a[$1] " " $2} END{for (m in a) print "   ", m, a[m], "us"}'
echo "    load $(cut -d' ' -f1-3 /proc/loadavg)"
