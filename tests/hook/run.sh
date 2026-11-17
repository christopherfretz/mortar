#!/bin/bash
# WP-077: the matrix of spec/odr.md §10.3 with prototype 3 (mason_odr_rt.cpp).
# usage: run.sh WORK TARGET [FLAVOR] [CASES]
#   TARGET = aarch64 (native) | x86_64 | i686 (cross GCC + qemu-user)
#   FLAVOR = hod (default: the runtime object kiln+hod build) | gcc (GCC -O2)
#   CASES  = a space-separated list (default: all), e.g. "A B R1m"
# Needs MORTAR (the mortar binary) and XTASK (the xtask binary) in the
# environment. Not run by any lane. Every line "MATRIX ..." is one row.
set -u
W=${1:?WORK}; T=${2:?TARGET}; RT_FLAVOR=${3:-hod}; CASES=${4:-all}
H=$(cd "$(dirname "$0")" && pwd)
: "${MORTAR:?}" "${XTASK:?}"
mkdir -p "$W"; W=$(cd "$W" && pwd)
. "$H/lib.sh"
want() { [ "$CASES" = all ] || [[ " $CASES " == *" $1 "* ]]; }
build_rt || { echo "runtime build failed"; exit 1; }

# hosts: GCC-linked (foreign: no table, no runtime) and mortar-linked Mason hosts
$CC -O2 "$H/host.c" -o "$W/host" -ldl -Wl,-rpath,'$ORIGIN'
$CC -O2 -c "$H/host.c" -o "$W/host.o"
# mhost: the runtime (executable flavour), report default, no vague definition
$CC "${LDM[@]}" "$W/host.o" "$RTEXE" "$W/note-report.o" -Wl,-z,now -o "$W/mhost" -ldl -Wl,-rpath,'$ORIGIN'
$CC "${LDM[@]}" "$W/host.o" "$RTEXE" "$W/note-fatal.o" -Wl,-z,now -o "$W/mhostf" -ldl -Wl,-rpath,'$ORIGIN'
# mhostv: a Mason host that defines and exports fmtlite::version (body 1), with its row
printf 'namespace fmtlite { inline int version() { return 1; } }\nextern "C" int keep() { return fmtlite::version(); }\n' > "$W/hx.cpp"
$CXX -O0 -c "$W/hx.cpp" -o "$W/hx.0.o" && meta "$W/hx.0.o" "$W/hx.o" "_ZN7fmtlite7versionEv=1"
$CXX "${LDM[@]}" "$W/host.o" "$W/hx.o" "$RTEXE" "$W/note-report.o" -Wl,--export-dynamic -Wl,--odr-table=exported -Wl,-z,now -o "$W/mhostv" -ldl -Wl,-rpath,'$ORIGIN'
$CXX "${LDM[@]}" "$W/host.o" "$W/hx.o" "$RTEXE" "$W/note-fatal.o" -Wl,--export-dynamic -Wl,--odr-table=exported -Wl,-z,now -o "$W/mhostvf" -ldl -Wl,-rpath,'$ORIGIN'

plug plain1 1 plain; plug plain2 2 plain
plug h1 1 rt; plug h1b 1 rt; plug h2 2 rt
plug h1lazy 1 rtlazy; plug h2lazy 2 rtlazy
plug h1n 1 rt; plug h2n 2 rt   # the same bodies, for the R3m variant with -z now
plug t1 1 table; plug t2 2 table

P=$W
if want A; then
  echo "== A1 plain plugins (no runtime), RTLD_LOCAL: each binds its own copy"
  r A1 -- ./host o:$P/plain1.so: c:$P/plain1.so o:$P/plain2.so: c:$P/plain2.so; verdict A1 none
  echo "== A2 runtime plugins, RTLD_LOCAL, foreign host"
  r A2 MASON_ODR_STATS=1 -- ./host o:$P/h1.so: c:$P/h1.so o:$P/h2.so: c:$P/h2.so; verdict A2 none
  echo "== A3 runtime plugins, RTLD_LOCAL, Mason host (fatal default)"
  r A3 MASON_ODR_STATS=1 -- ./mhostf o:$P/h1.so: c:$P/h1.so o:$P/h2.so: c:$P/h2.so; verdict A3 none
fi
if want E; then
  echo "== E h1 GLOBAL in the base namespace, h2 in a dlmopen(LM_ID_NEWLM) namespace, Mason host (fatal)"
  r E MASON_ODR_STATS=1 -- ./mhostf o:$P/h1.so:g c:$P/h1.so m:$P/h2.so; verdict E none
fi
if want F; then
  echo "== F h1 GLOBAL, dlclose, then h2 GLOBAL, Mason host (fatal)"
  r F MASON_ODR_STATS=1 -- ./mhostf o:$P/h1.so:g x:$P/h1.so o:$P/h2.so:g c:$P/h2.so; verdict F none
fi
if want C; then
  echo "== C2 h2 LOCAL, then h1 GLOBAL (h2 already bound its own), Mason host (fatal)"
  r C2 MASON_ODR_STATS=1 -- ./mhostf o:$P/h2.so: o:$P/h1.so:g c:$P/h2.so c:$P/h1.so; verdict C2 none
  echo "== C1 h1 GLOBAL, then h2 LOCAL (binds h1's): report"
  r C1 -- ./mhost o:$P/h1.so:g o:$P/h2.so: c:$P/h2.so; verdict C1 report
  echo "== C1 foreign host"
  r C1-foreign -- ./host o:$P/h1.so:g o:$P/h2.so: c:$P/h2.so; verdict C1-foreign report
  echo "== C1 fatal (Mason host, fatal default): the process stops at h2's load"
  r C1-fatal -- ./mhostf o:$P/h1.so:g o:$P/h2.so: c:$P/h2.so; verdict C1-fatal report
fi
if want R1m; then
  echo "== R1m h2 LOCAL|NOW (own), h1 GLOBAL, h1b LOCAL (binds h1's: same body), Mason host (fatal)"
  r R1m MASON_ODR_STATS=1 -- ./mhostf o:$P/h2.so: c:$P/h2.so o:$P/h1.so:g o:$P/h1b.so: c:$P/h1b.so c:$P/h2.so; verdict R1m none
fi
if want R2m; then
  echo "== R2m promotion: h2 LOCAL, h1 LOCAL, h1 NOLOAD|GLOBAL, h1b LOCAL, Mason host (fatal)"
  r R2m MASON_ODR_STATS=1 -- ./mhostf o:$P/h2.so: o:$P/h1.so: o:$P/h1.so:gn o:$P/h1b.so: c:$P/h2.so c:$P/h1.so c:$P/h1b.so; verdict R2m none
fi
if want R8m; then
  echo "== R8m h1 GLOBAL, h2 LOCAL|DEEPBIND (binds its own), h1b LOCAL, Mason host (fatal)"
  r R8m MASON_ODR_STATS=1 -- ./mhostf o:$P/h1.so:g o:$P/h2.so:d c:$P/h2.so o:$P/h1b.so: c:$P/h1b.so; verdict R8m none
fi
if want R3m; then
  echo "== R3m (as the re-review built it: -z lazy) h2lazy LOCAL|LAZY, h1lazy GLOBAL|LAZY, call h2lazy: binds h1's body after init"
  r R3m-lazy MASON_ODR_STATS=1 -- ./mhost o:$P/h2lazy.so:l o:$P/h1lazy.so:gl c:$P/h2lazy.so
  echo "MATRIX $T $RT_FLAVOR R3m-lazy required=report-or-exclusion observed: $LAST => $(echo "$LAST" | grep -q 'other-warnings=[1-9]' && echo 'pass (named exclusion: odr-hook-lazy)' || echo FAIL)"
  echo "== R3m with -z now (what the option links): the same opens, RTLD_LAZY asked; the module binds at load"
  r R3m-now MASON_ODR_STATS=1 -- ./mhostf o:$P/h2n.so:l o:$P/h1n.so:gl c:$P/h2n.so; verdict R3m-now none
fi
if want B; then
  echo "== B1 Mason host exports version() body 1; h2 LOCAL binds it: report"
  r B1 -- ./mhostvf o:$P/h2.so: c:$P/h2.so; verdict B1 report
  echo "== B2 the same in report mode: the bug is visible (h2 returns 1)"
  r B2 -- ./mhostv o:$P/h2.so: c:$P/h2.so; verdict B2 report
fi
if want D; then
  printf 'namespace fmtlite { inline int version() { return 3; } }\nextern "C" int dep_entry() { return fmtlite::version(); }\n' > "$W/e.cpp"
  $CXX -O0 -fPIC -c "$W/e.cpp" -o "$W/e.0.o" && meta "$W/e.0.o" "$W/e.o" "_ZN7fmtlite7versionEv=3"
  $CXX "${LDM[@]}" -shared "$W/e.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -Wl,-soname,libdepe.so -o "$W/libdepe.so"
  printf 'namespace fmtlite { inline int version() { return 4; } }\nextern "C" int dep_entry();\nextern "C" int plugin_entry() { return fmtlite::version() * 10 + dep_entry(); }\n' > "$W/d.cpp"
  $CXX -O0 -fPIC -c "$W/d.cpp" -o "$W/d.0.o" && meta "$W/d.0.o" "$W/d.o" "_ZN7fmtlite7versionEv=4"
  $CXX "${LDM[@]}" -shared "$W/d.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -L"$W" -ldepe -Wl,-rpath,'$ORIGIN' -o "$W/plugd.so"
  echo "== D plugd LOCAL, its dependency libdepe defines the key differently and binds plugd's copy: report"
  r D -- ./host o:$P/plugd.so: c:$P/plugd.so; verdict D report
fi
