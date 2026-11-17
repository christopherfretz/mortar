#!/bin/bash
# The load-time ODR matrix of spec/odr.md §10.3 (O-58; records WP-077, WP-196),
# on the runtime of runtime/odr/ (lib.sh: the checked-in objects by default).
# usage: run.sh WORK TARGET [FLAVOR] [CASES]
#   TARGET = aarch64 (native) | x86_64 | i686 (cross GCC + qemu-user)
#   FLAVOR = hod (default: the runtime object kiln+hod build) | gcc (GCC -O2)
#   BIG=1 adds RB-1M (a 1,000,000-row table: minutes of compile)
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
mlink $CC "$W/host.o" "$RTEXE" "$W/note-report.o" -Wl,-z,now -o "$W/mhost" -ldl -Wl,-rpath,'$ORIGIN'
mlink $CC "$W/host.o" "$RTEXE" "$W/note-fatal.o" -Wl,-z,now -o "$W/mhostf" -ldl -Wl,-rpath,'$ORIGIN'
# mhostv: a Mason host that defines and exports fmtlite::version (body 1), with its row
printf 'namespace fmtlite { inline int version() { return 1; } }\nextern "C" int keep() { return fmtlite::version(); }\n' > "$W/hx.cpp"
$CXX -O0 -c "$W/hx.cpp" -o "$W/hx.0.o" && meta "$W/hx.0.o" "$W/hx.o" "_ZN7fmtlite7versionEv=1"
mlink $CXX "$W/host.o" "$W/hx.o" "$RTEXE" "$W/note-report.o" -Wl,--export-dynamic -Wl,--odr-table=exported -Wl,-z,now -o "$W/mhostv" -ldl -Wl,-rpath,'$ORIGIN'
mlink $CXX "$W/host.o" "$W/hx.o" "$RTEXE" "$W/note-fatal.o" -Wl,--export-dynamic -Wl,--odr-table=exported -Wl,-z,now -o "$W/mhostvf" -ldl -Wl,-rpath,'$ORIGIN'

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
  mlink $CXX -shared "$W/e.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -Wl,-soname,libdepe.so -o "$W/libdepe.so"
  printf 'namespace fmtlite { inline int version() { return 4; } }\nextern "C" int dep_entry();\nextern "C" int plugin_entry() { return fmtlite::version() * 10 + dep_entry(); }\n' > "$W/d.cpp"
  $CXX -O0 -fPIC -c "$W/d.cpp" -o "$W/d.0.o" && meta "$W/d.0.o" "$W/d.o" "_ZN7fmtlite7versionEv=4"
  mlink $CXX -shared "$W/d.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -L"$W" -ldepe -Wl,-rpath,'$ORIGIN' -o "$W/plugd.so"
  echo "== D plugd LOCAL, its dependency libdepe defines the key differently and binds plugd's copy: report"
  r D -- ./host o:$P/plugd.so: c:$P/plugd.so; verdict D report
fi
if want R6; then
  for v in 1 2; do
    printf 'namespace fmtlite { inline int version() { return %d; } }\nextern "C" int e%d() { return fmtlite::version(); }\n' $v $v > "$W/v$v.cpp"
    printf 'LIB%d_1 { global: *; };\n' $v > "$W/v$v.map"
    $CXX -O0 -fPIC -c "$W/v$v.cpp" -o "$W/v$v.0.o" && meta "$W/v$v.0.o" "$W/v$v.o" "_ZN7fmtlite7versionEv=$v"
    mlink $CXX -shared "$W/v$v.o" "$RT" "$W/note-report.o" -Wl,--version-script="$W/v$v.map" -Wl,--odr-table=exported -Wl,-z,now -Wl,-soname,libv$v.so -o "$W/libv$v.so"
  done
  printf '#include <stdio.h>\nextern "C" int e1(); extern "C" int e2();\nint main(){ printf("e1=%%d e2=%%d\\n", e1(), e2()); }\n' > "$W/v.cpp"
  $CXX "$W/v.cpp" -Wl,--no-as-needed -L"$W" -lv1 -lv2 -Wl,-rpath,'$ORIGIN' -o "$W/vhook"
  $CXX -c "$W/v.cpp" -o "$W/v.o"
  mlink $CXX "$W/v.o" "$RTEXE" "$W/note-fatal.o" -Wl,-z,now -Wl,--no-as-needed -L"$W" -lv1 -lv2 -Wl,-rpath,'$ORIGIN' -o "$W/vmhook"
  echo "== R6 libv1 (LIB1_1) and libv2 (LIB2_1), different version(); foreign executable (vhook): checked, nothing to report"
  r R6-vhook MASON_ODR_STATS=1 -- ./vhook; verdict R6-vhook none
  echo "== R6 Mason executable (vmhook, fatal)"
  r R6-vmhook MASON_ODR_STATS=1 -- ./vmhook; verdict R6-vmhook none
fi
if want R4; then
  printf 'namespace fmtlite { inline int version() { return 1; } }\nextern "C" int x_entry() { return fmtlite::version(); }\n' > "$W/x.cpp"
  printf 'namespace fmtlite { __attribute__((visibility("protected"))) inline int version() { return 1; } }\nextern "C" int x_entry() { return fmtlite::version(); }\n' > "$W/xp.cpp"
  $CXX -O0 -fPIC -c "$W/x.cpp" -o "$W/x.0.o" && meta "$W/x.0.o" "$W/x.o" "_ZN7fmtlite7versionEv=1"
  $CXX -O0 -fPIC -c "$W/xp.cpp" -o "$W/xp.0.o" && meta "$W/xp.0.o" "$W/xp.o" "_ZN7fmtlite7versionEv=1"
  mlink $CXX -shared "$W/x.o" -Wl,-Bsymbolic -Wl,--odr-table=exported -Wl,-soname,libxb.so -o "$W/libxb.so"
  mlink $CXX -shared "$W/x.o" -Wl,-Bsymbolic-functions -Wl,--odr-table=exported -Wl,-soname,libxbf.so -o "$W/libxbf.so"
  mlink $CXX -shared "$W/xp.o" -Wl,--odr-table=exported -Wl,-soname,libxp.so -o "$W/libxp.so"
  printf 'namespace fmtlite { inline int version() { return 2; } }\nextern "C" int y_entry() { return fmtlite::version(); }\n' > "$W/y.cpp"
  $CXX -O0 -fPIC -c "$W/y.cpp" -o "$W/y.0.o" && meta "$W/y.0.o" "$W/y.o" "_ZN7fmtlite7versionEv=2"
  mlink $CXX -shared "$W/y.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -Wl,-soname,liby.so -o "$W/liby.so"
  printf '#include <stdio.h>\nextern "C" int x_entry(); extern "C" int y_entry();\nint main(){ printf("x=%%d y=%%d (y expects 2)\\n", x_entry(), y_entry()); }\n' > "$W/xy.cpp"
  for v in xb xbf xp; do
    $CXX "$W/xy.cpp" -Wl,--no-as-needed -L"$W" -l$v -ly -Wl,-rpath,'$ORIGIN' -o "$W/xy-$v"
    echo "== R4/R5 lib$v (version() 1, NONPREEMPTIBLE row: $(readelf -nW "$W/lib$v.so" | grep -o '0x4d4f4401.*' | awk '{print $NF, $(NF-1)}')) before liby (2, runtime): liby binds lib$v's: report"
    r R45-$v -- ./xy-$v; verdict R45-$v report
  done
fi
if want VER; then
  # the test producer's table (gcc + GNU ld + a placeholder note, filled after the link):
  # libnv defines f@V1 (body 21) and f@@V2 (body 22); mortar's table for libp (f unversioned)
  printf 'int f1(void) { return 21; }\nint f2(void) { return 22; }\n__asm__(".symver f1,f@V1");\n__asm__(".symver f2,f@@V2");\n' > "$W/nv.c"
  printf 'V1 { global: f; local: *; };\nV2 { global: f; } V1;\n' > "$W/nv.map"
  printf '\t.section .note.mason,"a",%%note\n\t.p2align 3\n\t.long 10\n\t.long 64\n\t.long 0x4d4f4401\n\t.asciz "MasonMeta"\n\t.p2align 3\n\t.long 1, 2, 24, 0\n\t.zero 48\n\t.section .note.GNU-stack,"",%%progbits\n' > "$W/note2.s"
  $CC -O1 -fPIC -c "$W/nv.c" -o "$W/nv.o" && $CC -c "$W/note2.s" -o "$W/note2.o"
  $CC -shared "$W/nv.o" "$W/note2.o" "$RT" "$W/note-report.o" -Wl,-z,now -Wl,--version-script="$W/nv.map" -Wl,-soname,libnv.so -o "$W/libnv0.so"
  printf 'row name=f version=V1 hash=%s flags=1\nrow name=f version=V2 hash=%s flags=1\n' "$(hash_of 21 | cut -c1-16)" "$(hash_of 22 | cut -c1-16)" > "$W/nv.rows"
  "$XTASK" meta table "$W/nv.rows" --into "$W/libnv0.so" -o "$W/libnv.so"
  for b in 21 22; do
    printf 'int f(void) { return %d; }\nint fp%d(void) { return f(); }\n' $b $b > "$W/p$b.c"
    $CC -O0 -fPIC -c "$W/p$b.c" -o "$W/p$b.0.o" && meta "$W/p$b.0.o" "$W/p$b.o" "f=$b"
    mlink $CC -shared "$W/p$b.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -Wl,-soname,libp$b.so -o "$W/libp$b.so"
    printf '#include <stdio.h>\nint fp%d(void);\nint main(void) { printf("fp=%%d\\n", fp%d()); return 0; }\n' $b $b > "$W/vm$b.c"
    $CC "$W/vm$b.c" -Wl,--no-as-needed -L"$W" -lnv -lp$b -Wl,-rpath,'$ORIGIN' -o "$W/vm$b"
  done
  echo "== VER-a libnv (f@V1 21, f@@V2 22) first, libp21 (f 21, unversioned): libp binds f@V1 (glibc: index 2), same body"
  r VER-a MASON_ODR_STATS=1 -- ./vm21; verdict VER-a none
  echo "== VER-b the same with libp22 (f 22): binds f@V1 (21), while the default f@@V2 is 22: report"
  r VER-b -- ./vm22; verdict VER-b report
fi
if want CR; then
  # a copy relocation: a non-PIE executable reads the library's vague variable, so the
  # library's own GLOB_DAT binds the executable's copy (which has no row)
  printf 'namespace fmtlite { inline int level = 5; }\nextern "C" int lib_level() { return fmtlite::level; }\n' > "$W/cr.cpp"
  $CXX -O0 -fPIC -c "$W/cr.cpp" -o "$W/cr.0.o" && meta "$W/cr.0.o" "$W/cr.o" "_ZN7fmtlite5levelE=5"
  mlink $CXX -shared "$W/cr.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -Wl,-soname,libcr.so -o "$W/libcr.so"
  printf '#include <stdio.h>\nextern int lvl __asm__("_ZN7fmtlite5levelE");\nint lib_level(void);\nint main(void) { lvl = 7; printf("exe %%d lib %%d\\n", lvl, lib_level()); return lib_level() != 7; }\n' > "$W/crm.c"
  $CC -O0 -fno-pic -c "$W/crm.c" -o "$W/crm.o"
  $CC -no-pie "$W/crm.o" -Wl,--no-as-needed -L"$W" -lcr -Wl,-rpath,'$ORIGIN' -o "$W/crmain-foreign"
  mlink $CC -no-pie "$W/crm.o" "$RTEXE" "$W/note-fatal.o" -Wl,--odr-table=exported -Wl,-z,now -Wl,--no-as-needed -L"$W" -lcr -Wl,-rpath,'$ORIGIN' -o "$W/crmain"
  echo "== CR copy relocation, foreign non-PIE executable: libcr binds the executable's copy"
  r CR-foreign MASON_ODR_STATS=1 -- ./crmain-foreign; verdict CR-foreign none
  echo "== CR the same, Mason non-PIE executable (fatal)"
  r CR-mason MASON_ODR_STATS=1 -- ./crmain; verdict CR-mason none
  # CR-b: a second library defines level differently (9); its reference also binds the
  # executable's copy, which holds libcr's definition (5): a violation the copy hides
  printf 'namespace fmtlite { inline int level = 9; }\nextern "C" int lib2_level() { return fmtlite::level; }\n' > "$W/cr2.cpp"
  $CXX -O0 -fPIC -c "$W/cr2.cpp" -o "$W/cr2.0.o" && meta "$W/cr2.0.o" "$W/cr2.o" "_ZN7fmtlite5levelE=9"
  mlink $CXX -shared "$W/cr2.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -Wl,-soname,libcr2.so -o "$W/libcr2.so"
  printf '#include <stdio.h>\nextern int lvl __asm__("_ZN7fmtlite5levelE");\nint lib2_level(void);\nint main(void) { printf("exe %%d lib2 %%d (lib2 expects 9)\\n", lvl, lib2_level()); return 0; }\n' > "$W/crm2.c"
  $CC -O0 -fno-pic -c "$W/crm2.c" -o "$W/crm2.o"
  mlink $CC -no-pie "$W/crm2.o" "$RTEXE" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -Wl,--no-as-needed -L"$W" -lcr -lcr2 -Wl,-rpath,'$ORIGIN' -o "$W/crmain2"
  echo "== CR-b libcr (5) then libcr2 (9), Mason non-PIE executable: libcr2 binds the copy of libcr's level"
  r CR-b MASON_ODR_STATS=1 -- ./crmain2
  echo "MATRIX $T $RT_FLAVOR CR-b required=report-or-exclusion observed: $LAST => $(echo "$LAST" | grep -q 'reports=[1-9]' && echo pass || echo 'excluded (copy: the executable'"'"'s copy has no row; named exclusion odr-copy-unchecked proposed)')"
fi
if want P; then
  plug pre 1 table
  plug prel 1 plain
  echo "== P1 LD_PRELOAD of a Mason library (table, version() 1); h2 LOCAL binds it; Mason host, fatal default: a warning, the process goes on"
  r P1 LD_PRELOAD=$W/pre.so -- ./mhostf o:$P/h2.so: c:$P/h2.so
  echo "MATRIX $T $RT_FLAVOR P1 required=interposition-warning,no-stop observed: $LAST => $(echo "$LAST" | grep -q 'reports=0 .*exit=0' && echo pass || echo FAIL) (interposition warnings: $(cd "$W" && xrun LD_PRELOAD=$W/pre.so -- ./mhostf o:$P/h2.so: c:$P/h2.so 2>&1 | grep -c odr-interposed-at-load))"
  echo "== P2 the same under MASON_ODR=fatal,strict: the interposition is a mismatch"
  r P2 LD_PRELOAD=$W/pre.so MASON_ODR=fatal,strict -- ./mhostf o:$P/h2.so: c:$P/h2.so; verdict P2 report
  echo "== P3 LD_PRELOAD of a foreign library (no table): not checked (O-56)"
  r P3 LD_PRELOAD=$W/prel.so MASON_ODR_STATS=1 -- ./mhostf o:$P/h2.so: c:$P/h2.so; verdict P3 none
fi
if want T; then
  echo "== T1 a runtime module binds a table-only module's different definition (t1 GLOBAL, h2 LOCAL): report"
  r T1 -- ./mhost o:$P/t1.so:g o:$P/h2.so: c:$P/h2.so; verdict T1 report
  echo "== T2 a table-only module binds a runtime module's definition after dlopen (h1 GLOBAL, t2 LOCAL): not checked"
  r T2 MASON_ODR_STATS=1 -- ./mhost o:$P/h1.so:g o:$P/t2.so: c:$P/t2.so
  echo "MATRIX $T $RT_FLAVOR T2-dlopen required=named-exclusion observed: $LAST => $(echo "$LAST" | grep -q 'reports=0' && echo 'excluded (t2 has no runtime and nothing checks it)' || echo reported)"
  # the initial set: liba1 (runtime, 1) then libat2 (table only, 2) as DT_NEEDED of a Mason executable
  plug a1 1 rt -Wl,-soname,liba1.so; cp "$W/a1.so" "$W/liba1.so"
  plug at2 2 table -Wl,-soname,libat2.so; cp "$W/at2.so" "$W/libat2.so"
  plug atn2 2 table -Wl,-z,now -Wl,-soname,libatn2.so; cp "$W/atn2.so" "$W/libatn2.so"
  printf 'extern "C" int entry_a1(); extern "C" int entry_at2();\n#include <stdio.h>\nint main(){ printf("a1=%%d at2=%%d\\n", entry_a1(), entry_at2()); }\n' > "$W/tm.cpp"
  $CXX -c "$W/tm.cpp" -o "$W/tm.o"
  mlink $CXX "$W/tm.o" "$RTEXE" "$W/note-report.o" -Wl,-z,now -Wl,--no-as-needed -L"$W" -la1 -lat2 -Wl,-rpath,'$ORIGIN' -o "$W/tmain"
  printf 'extern "C" int entry_a1(); extern "C" int entry_atn2();\n#include <stdio.h>\nint main(){ printf("a1=%%d atn2=%%d\\n", entry_a1(), entry_atn2()); }\n' > "$W/tmn.cpp"
  $CXX -c "$W/tmn.cpp" -o "$W/tmn.o"
  mlink $CXX "$W/tmn.o" "$RTEXE" "$W/note-report.o" -Wl,-z,now -Wl,--no-as-needed -L"$W" -la1 -latn2 -Wl,-rpath,'$ORIGIN' -o "$W/tmainn"
  echo "== T2 initial set: libatn2 (table only, -z now) binds liba1's definition; the executable's .preinit_array entry checks it: report"
  r T2-initial-now MASON_ODR_STATS=1 -- ./tmainn; verdict T2-initial-now report
  echo "== T2 initial set, libat2 table only and lazy (mortar's default): the .preinit_array entry says it cannot check it"
  r T2-initial-lazy MASON_ODR_STATS=1 -- ./tmain
  echo "MATRIX $T $RT_FLAVOR T2-initial-lazy required=report-or-exclusion observed: $LAST => $(echo "$LAST" | grep -q 'other-warnings=[1-9]' && echo 'pass (named exclusion: odr-hook-lazy)' || echo FAIL)"
fi
build_s() { # the S set: libs1 (1), libs2 (2, a constructor) bind as one program
  [ -f "$W/smain-fatal" ] && return 0
  plug s1 1 rt -Wl,-soname,libs1.so; cp "$W/s1.so" "$W/libs1.so"
  printf 'namespace fmtlite { inline int version() { return 2; } }\n#include <unistd.h>\n__attribute__((constructor)) static void c() { write(1, "ctor s2\\n", 8); }\nextern "C" int entry_s2() { return fmtlite::version(); }\n' > "$W/s2.cpp"
  $CXX -O0 -fPIC -c "$W/s2.cpp" -o "$W/s2.0.o" && meta "$W/s2.0.o" "$W/s2.o" "_ZN7fmtlite7versionEv=2"
  mlink $CXX -shared "$W/s2.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -Wl,-soname,libs2.so -o "$W/libs2.so"
  printf 'extern "C" int entry_s1(); extern "C" int entry_s2();\n#include <stdio.h>\nint main(){ printf("main s1=%%d s2=%%d\\n", entry_s1(), entry_s2()); }\n' > "$W/sm.cpp"
  $CXX -c "$W/sm.cpp" -o "$W/sm.o"
  for m in report fatal; do
    mlink $CXX "$W/sm.o" "$RTEXE" "$W/note-$m.o" -Wl,-z,now -Wl,--no-as-needed -L"$W" -ls1 -ls2 -Wl,-rpath,'$ORIGIN' -o "$W/smain-$m"
  done
  $CXX "$W/sm.o" -Wl,--no-as-needed -L"$W" -ls1 -ls2 -Wl,-rpath,'$ORIGIN' -o "$W/smain-foreign"
}
if want S; then
  # fatal at program start: nothing user-written runs before the stop
  build_s
  echo "== S-fatal libs2 binds libs1's version(), Mason executable fatal: reported by .preinit_array, stopped before 'ctor s2'"
  r S-fatal MASON_ODR_STATS=1 -- ./smain-fatal
  echo "MATRIX $T $RT_FLAVOR S-fatal required=report,stop-before-constructors observed: $LAST => $(echo "$LAST" | grep -q 'reports=1 .*exit=134' && ! grep -q 'ctor s2' <<< "$(cd "$W" && "${RUN[@]}" ./smain-fatal 2>&1)" && echo pass || echo FAIL)"
  echo "== S-report the same, report default: one report (libs2's own entry), then the program runs"
  r S-report -- ./smain-report; verdict S-report report
  echo "== S-foreign a foreign executable, libraries report by themselves"
  r S-foreign -- ./smain-foreign; verdict S-foreign report
  echo "== S-env MASON_ODR=fatal over the report default"
  r S-env MASON_ODR=fatal -- ./smain-report; verdict S-env report
  echo "== S-off MASON_ODR=off over the fatal default"
  r S-off MASON_ODR=off -- ./smain-fatal; verdict S-off none
fi
if want G; then
  many 300 1000 bad g
  echo "== G 300 modules x 1,250 rows, all DT_NEEDED, module 300 changes mk7: report"
  r G MASON_ODR=report -- ./many-g/many; verdict G report
  timeit "$W/many-g" "G 300 x 1,250, one violating key"
fi
if want H; then
  many 2 20000 bad h
  echo "== H a module of 25,000 rows (above the 4,096-symbol stack map): mmap'd map, then mmap refused (slow path, no memory)"
  r H-map MASON_ODR_STATS=1 -- ./many-h/many; verdict H-map report
  r H-nommap MASON_ODR_STATS=1 MASON_ODR_TEST_NOMMAP=1 -- ./many-h/many; verdict H-nommap report
fi
if want I; then
  printf '#include <unistd.h>\n__attribute__((constructor(101))) static void u101(void) { write(2, "user constructor(101)\\n", 22); }\n__attribute__((constructor)) static void udef(void) { write(2, "user constructor (default)\\n", 27); }\n' > "$W/uc.c"
  echo 'int main(void){ return 0; }' > "$W/um.c"
  for l in bfd lld mold mortar; do
    if [ $l = mortar ]; then LF=("${LDM[@]}"); else LF=(-fuse-ld=$l); fi
    [ "$T" = i686 ] && [ $l = mold ] && continue
    [ "$PRODUCER" != mortar ] && [ $l = mortar ] && continue
    $CC -O1 -fPIC -shared "${LF[@]}" "$W/uc.c" "$RT" "$W/note-report.o" -Wl,-z,now -o "$W/libuc-$l.so" -Wl,-soname,libuc-$l.so || continue
    $CC "${LF[@]}" "$W/um.c" -Wl,--no-as-needed -L"$W" -luc-$l -Wl,-rpath,'$ORIGIN' -o "$W/uc-$l"
    $CC -O1 "${LF[@]}" "$W/uc.c" "$W/um.c" "$RTEXE" "$W/note-report.o" -Wl,-z,now -o "$W/uce-$l"
    so=$(cd "$W" && xrun MASON_ODR_STATS=1 -- ./uc-$l 2>&1 | sed 's/ \/.*//' | tr '\n' ',')
    ex=$(cd "$W" && xrun MASON_ODR_STATS=1 -- ./uce-$l 2>&1 | sed 's/ \/.*//; s/ (main program)//' | tr '\n' ',')
    echo "  $l .so: $so"
    echo "  $l executable: $ex"
    ok=FAIL; [[ $so == mason-entry,user\ constructor\(101\),* ]] && [[ $ex == mason-preinit,user\ constructor\(101\),* ]] && ok=pass
    echo "MATRIX $T $RT_FLAVOR I-$l required=runtime-first observed: .so [$so] exe [$ex] => $ok"
  done
fi
if want J; then
  mkdir -p "$W/thr"
  for i in 1 2 3 4; do
    python3 "$H/gen_pool.py" "$W/thr/j$i" 10000 "j$i"
    $CC -O1 -fPIC -c "$W/thr/j$i.c" -o "$W/thr/j$i.0.o" && meta_file "$W/thr/j$i.meta" "$W/thr/j$i.0.o" "$W/thr/j$i.o"
    mlink $CC -shared "$W/thr/j$i.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -o "$W/thr/libj$i.so"
  done
  $CC -O2 "$H/threads.c" -o "$W/thr/st" -ldl -lpthread
  for k in 1 2 3; do
    s=$(date +%s%N)
    out=$(cd "$W" && timeout 600 "${RUN[@]}" ./thr/st "$W/thr" 2>&1); st=$?; out=$(echo "$out" | tr '\n' ' ')
    echo "MATRIX $T $RT_FLAVOR J-run$k required=no-crash,no-hang,no-report observed: exit=$st [$out] $(( ($(date +%s%N) - s) / 1000000 )) ms => $([ "$out" = "done " ] && echo pass || echo FAIL)"
  done
fi
if want Q; then
  K=10; NQ=${NQ:-300}
  quad "$NQ" $K
  L=(); for i in $(seq 1 "$NQ"); do L+=("o:$W/q/p$i.so:"); done
  echo "== Q $NQ RTLD_LOCAL plugins, $K private keys each, Mason host (fatal): no report, cost"
  out=$(cd "$W" && xrun MASON_ODR_TIME=1 -- ./mhostf "${L[@]}" 2>&1); st=$?
  LAST="reports=$(echo "$out" | grep -c ': odr-violation-at-load: [^0-9]') other-warnings=$(echo "$out" | grep -c 'warning: odr-hook') exit=$st"
  echo "    $(echo "$out" | awk '/mason-time/{s+=$2; n++} END{printf "entries=%d summed check time=%.2f ms", n, s/1e6}')"
  verdict Q none
fi
if want CP; then
  # a canonical PLT: a non-PIE, non-PIC executable takes the address of version(),
  # which libcp defines, so the executable's .dynsym holds version undefined with a
  # value (its PLT entry) and libcp's own GLOB_DAT binds that address
  for v in 2 3; do
    printf 'namespace fmtlite { inline int version() { return %d; } }\nextern "C" void* cp%d_addr() { return (void*)&fmtlite::version; }\nextern "C" int cp%d_call() { return fmtlite::version(); }\n' $v $v $v > "$W/cp$v.cpp"
    $CXX -O0 -fPIC -c "$W/cp$v.cpp" -o "$W/cp$v.0.o" && meta "$W/cp$v.0.o" "$W/cp$v.o" "_ZN7fmtlite7versionEv=$v"
    mlink $CXX -shared "$W/cp$v.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -Wl,-soname,libcp$v.so -o "$W/libcp$v.so"
  done
  printf '#include <stdio.h>\nextern int fv(void) __asm__("_ZN7fmtlite7versionEv");\nvoid* cp2_addr(void);\nint main(void) { void* p = (void*)&fv; printf("exe==lib %%d call %%d\\n", p == cp2_addr(), fv()); return p != cp2_addr(); }\n' > "$W/cpm.c"
  printf '#include <stdio.h>\nextern int fv(void) __asm__("_ZN7fmtlite7versionEv");\nint cp3_call(void);\nint main(void) { void* p = (void*)&fv; printf("exe %%d lib3 %%d (lib3 expects 3)\\n", ((int (*)(void))p)(), cp3_call()); return 0; }\n' > "$W/cpm3.c"
  $CC -O0 -fno-pic -c "$W/cpm.c" -o "$W/cpm.o"; $CC -O0 -fno-pic -c "$W/cpm3.c" -o "$W/cpm3.o"
  for l in bfd lld mold mortar; do
    if [ $l = mortar ]; then LF=("${LDM[@]}"); else LF=(-fuse-ld=$l); fi
    [ "$T" = i686 ] && [ $l = mold ] && continue
    [ "$PRODUCER" != mortar ] && [ $l = mortar ] && continue
    $CC -no-pie "${LF[@]}" "$W/cpm.o" -Wl,--no-as-needed -L"$W" -lcp2 -Wl,-rpath,'$ORIGIN' -o "$W/cpm-$l" || { echo "MATRIX $T $RT_FLAVOR CP-$l required=none observed: link failed => FAIL"; continue; }
    $CC -no-pie "${LF[@]}" "$W/cpm3.o" -Wl,--no-as-needed -L"$W" -lcp2 -lcp3 -Wl,-rpath,'$ORIGIN' -o "$W/cpm3-$l"
    sym=$(readelf -W --dyn-syms "$W/cpm-$l" | awk '$8 == "_ZN7fmtlite7versionEv" || $8 ~ /^_ZN7fmtlite7versionEv@/ { print $7 "/0x" $2; exit }')
    echo "== CP-$l libcp2 (2, runtime) binds the PLT entry of a non-PIE executable linked by $l (.dynsym version: ndx/value $sym)"
    r CP-$l MASON_ODR_STATS=1 -- ./cpm-$l
    ok=1; [[ $LAST == reports=0\ other-warnings=0\ exit=0 ]] && [ "$(stv canonical-plt)" -ge 1 ] && [ "$(stv no-row)" = 0 ] && ok=0
    row CP-$l none,counted-canonical-plt $ok "[canonical-plt=$(stv canonical-plt) same=$(stv same) no-row=$(stv no-row) dynsym=$sym]"
    echo "== CP-b-$l libcp2 (2) then libcp3 (3): libcp3's GLOB_DAT binds the PLT entry, which calls libcp2's: judged against libcp2: report"
    r CP-b-$l MASON_ODR_STATS=1 -- ./cpm3-$l
    ok=1; [[ $LAST == reports=1\ * ]] && ok=0
    row CP-b-$l report $ok "[canonical-plt=$(stv canonical-plt) differ=$(stv differ)]"
  done
fi
if want IF; then
  # an IFUNC with a row: libif defines fv as an IFUNC (body 2), refers to it by GLOB_DAT
  # (its address) and JUMP_SLOT (a call), and calls a hidden IFUNC (an IRELATIVE);
  # libifb defines fv plainly (body 2) and, loaded after libif GLOBAL, binds libif's
  printf 'static int impl(void) { return 2; }\nstatic void* res(void) { return (void*)impl; }\nint fv(void) __attribute__((ifunc("res")));\nstatic int himpl(void) { return 5; }\nstatic void* hres(void) { return (void*)himpl; }\n__attribute__((visibility("hidden"))) int hv(void) __attribute__((ifunc("hres")));\nvoid* if_addr(void) { return (void*)fv; }\nint plugin_entry(void) { return fv() * 10 + hv(); }\n' > "$W/if.c"
  printf 'int fv(void) { return 2; }\nvoid* ifb_addr(void) { return (void*)fv; }\nint plugin_entry(void) { return fv(); }\n' > "$W/ifb.c"
  for m in if ifb; do
    $CC -O0 -fPIC -c "$W/$m.c" -o "$W/$m.0.o" && meta "$W/$m.0.o" "$W/$m.o" "fv=2"
    mlink $CC -shared "$W/$m.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -Wl,-soname,lib$m.so -o "$W/lib$m.so"
  done
  rel=$(readelf -rW "$W/libif.so" | grep -o 'R_[A-Z0-9_]*\(GLOB_DAT\|JUMP_SLOT\|IRELATIVE\|IRELATIV\)' | sort | uniq -c | tr -s ' ' | tr '\n' ',')
  echo "== IF-own libif alone (relocations: $rel): its own IFUNC"
  r IF-own MASON_ODR_STATS=1 -- ./mhostf o:$W/libif.so: c:$W/libif.so
  ok=1; [[ $LAST == reports=0\ other-warnings=0\ exit=0 ]] && [ "$(stv ifunc)" -ge 1 ] && ok=0
  row IF-own none,counted-ifunc $ok "[ifunc=$(stv ifunc) own=$(stv own) unknown=$(stv unknown)]"
  echo "== IF-bind libif GLOBAL, then libifb LOCAL binds libif's IFUNC (the slot holds the resolver's result)"
  r IF-bind MASON_ODR_STATS=1 -- ./mhostf o:$W/libif.so:g o:$W/libifb.so: c:$W/libifb.so
  ok=1; [[ $LAST == reports=0\ other-warnings=0\ exit=0 ]] && [ "$(stv ifunc)" -ge 2 ] && ok=0
  row IF-bind none,counted-ifunc $ok "[ifunc=$(stv ifunc) no-row=$(stv no-row) unknown=$(stv unknown)]"
fi
if want TL; then
  # a thread_local inline variable: tl1 (5) GLOBAL, then tl2 (6) LOCAL binds tl1's
  # (a real difference the TLS relocations hide from this check: counted, tls=)
  for v in 1 2; do
    printf 'namespace fmtlite { inline thread_local int tl = %d; }\nextern "C" int plugin_entry() { return fmtlite::tl; }\n' $((v + 4)) > "$W/tl$v.cpp"
    $CXX -O0 -fPIC -c "$W/tl$v.cpp" -o "$W/tl$v.0.o" && meta "$W/tl$v.0.o" "$W/tl$v.o" "_ZN7fmtlite2tlE=$((v + 4))"
    mlink $CXX -shared "$W/tl$v.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -o "$W/tl$v.so"
  done
  rel=$(readelf -rW "$W/tl2.so" | grep -o 'R_[A-Z0-9_]*TLS[A-Z0-9_]*\|R_[A-Z0-9_]*DTP[A-Z0-9_]*\|R_[A-Z0-9_]*TPOFF[A-Z0-9_]*' | sort | uniq -c | tr -s ' ' | tr '\n' ',')
  echo "== TL tl1 GLOBAL, tl2 LOCAL binds tl1's thread_local (tl2's TLS relocations: $rel)"
  r TL MASON_ODR_STATS=1 -- ./mhostf o:$W/tl1.so:g c:$W/tl1.so o:$W/tl2.so: c:$W/tl2.so
  ok=1; [[ $LAST == reports=0\ other-warnings=0\ exit=0 ]] && [ "$(stv tls)" -ge 1 ] && ok=0
  row TL none,counted-tls $ok "[tls=$(stv tls)]"
fi
if want HV; then
  # hidden vague definitions (-fvisibility-inlines-hidden): two private copies, no binding
  for v in 1 2; do
    printf 'namespace fmtlite { inline int version() { return %d; } }\nextern "C" int plugin_entry() { return fmtlite::version(); }\n' $v > "$W/hv$v.cpp"
    # the unit has the row; mortar's =exported writes none for a hidden definition, and
    # the GNU route's table filler refuses one, so that route gets the unit without it
    hvrow="_ZN7fmtlite7versionEv=$v"; [ "$PRODUCER" = mortar ] || hvrow=""
    $CXX -O0 -fPIC -fvisibility-inlines-hidden -c "$W/hv$v.cpp" -o "$W/hv$v.0.o" && meta "$W/hv$v.0.o" "$W/hv$v.o" "$hvrow"
    mlink $CXX -shared "$W/hv$v.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -o "$W/hv$v.so"
  done
  echo "== HV hv1 GLOBAL, hv2 LOCAL: each its own hidden version(), nothing binds across"
  r HV MASON_ODR_STATS=1 -- ./mhostf o:$W/hv1.so:g c:$W/hv1.so o:$W/hv2.so: c:$W/hv2.so
  ok=1; [[ $LAST == reports=0\ other-warnings=0\ exit=0 ]] && printf '%s\n' "$LASTOUT" | grep -q 'hv2.so -> 2' && ok=0
  row HV none $ok "[rows=$(stv rows) relocs=$(stv relocs)]"
fi
if want SE; then
  # secure execution (AT_SECURE): MASON_ODR and the reading of LD_PRELOAD are ignored, the
  # notes decide. The seam MASON_ODR_TEST_SECURE=1 can only turn secure mode on.
  build_s
  [ -f "$W/pre.so" ] || plug pre 1 table
  echo "== SE-off MASON_ODR=off under secure execution, fatal default: still stops"
  r SE-off MASON_ODR_TEST_SECURE=1 MASON_ODR=off MASON_ODR_STATS=1 -- ./smain-fatal
  ok=1; [[ $LAST == reports=1\ other-warnings=0\ exit=134 ]] && [ "$(printf '%s\n' "$LASTOUT" | grep -c mason-stats)" = 0 ] && ok=0
  row SE-off report,stop,env-ignored $ok "[stats lines=$(printf '%s\n' "$LASTOUT" | grep -c mason-stats)]"
  echo "== SE-fatal MASON_ODR=fatal under secure execution, report default: reports, runs on"
  r SE-fatal MASON_ODR_TEST_SECURE=1 MASON_ODR=fatal -- ./smain-report
  ok=1; [[ $LAST == reports=1\ other-warnings=0\ exit=0 ]] && ok=0
  row SE-fatal report,env-ignored $ok
  echo "== SE-preload LD_PRELOAD of a Mason library under secure execution: not read as an interposition; the binding is judged (fatal)"
  r SE-preload LD_PRELOAD=$W/pre.so MASON_ODR_TEST_SECURE=1 -- ./mhostf o:$P/h2.so: c:$P/h2.so
  ok=1; [[ $LAST == reports=1\ * ]] && [[ $LAST == *exit=134 ]] && [ "$(warned odr-interposed-at-load)" = 0 ] && ok=0
  row SE-preload report,no-interposition-reading $ok "[odr-interposed-at-load: $(warned odr-interposed-at-load), odr-hook-lazy (pre.so is table-only and lazy): $(warned odr-hook-lazy)]"
  echo "== SE-seam MASON_ODR_TEST_SECURE=0 is not the seam (only 1 is): the process is not secure, MASON_ODR=off applies"
  r SE-seam MASON_ODR_TEST_SECURE=0 MASON_ODR=off -- ./smain-fatal
  ok=1; [[ $LAST == reports=0\ other-warnings=0\ exit=0 ]] && ok=0
  row SE-seam none $ok
fi
if want BM; then
  build_s
  echo "== BM-a MASON_ODR=fatl over the report default: one odr-hook-bad-mode, then the default"
  r BM-a MASON_ODR=fatl -- ./smain-report
  ok=1; [[ $LAST == reports=1\ other-warnings=1\ exit=0 ]] && [ "$(warned odr-hook-bad-mode)" = 1 ] && ok=0
  row BM-a report,bad-mode-warning $ok
  echo "== BM-b MASON_ODR=off,fatl over the fatal default: the warning, and off applies"
  r BM-b MASON_ODR=off,fatl -- ./smain-fatal
  ok=1; [[ $LAST == reports=0\ other-warnings=1\ exit=0 ]] && [ "$(warned odr-hook-bad-mode)" = 1 ] && ok=0
  row BM-b none,bad-mode-warning $ok
  echo "== BM-c MASON_ODR=fatl, a foreign executable: two libraries with the runtime, one warning (the first in load order speaks)"
  r BM-c MASON_ODR=fatl -- ./smain-foreign
  ok=1; [[ $LAST == reports=1\ other-warnings=1\ exit=0 ]] && [ "$(warned odr-hook-bad-mode)" = 1 ] && ok=0
  row BM-c report,bad-mode-warning $ok
fi
if want MT; then
  # a malformed table in ANOTHER module: mt (table only, version() 1 and other() 7) loaded
  # GLOBAL, then h2 (runtime, version() 2) LOCAL binds mt's version(); Mason host, fatal
  printf 'namespace fmtlite { inline int version() { return 1; } inline int other() { return 7; } }\nextern "C" int mt_entry() { return fmtlite::version() + fmtlite::other(); }\n' > "$W/mt.cpp"
  $CXX -O0 -fPIC -c "$W/mt.cpp" -o "$W/mt.0.o" && meta "$W/mt.0.o" "$W/mt.o" "_ZN7fmtlite7versionEv=1 _ZN7fmtlite5otherEv=7"
  mlink $CXX -shared "$W/mt.o" -Wl,--odr-table=exported -Wl,-z,now -o "$W/mt.so"
  echo "== MT-good the unpatched module: report, stop"
  r MT-good -- ./mhostf o:$W/mt.so:g o:$P/h2.so: c:$P/h2.so; verdict MT-good report
  for m in count trunc seg off sym order; do
    python3 "$H/mt_patch.py" "$W/mt.so" "$W/mt-$m.so" $m || { echo "MATRIX $T $RT_FLAVOR MT-$m required=malformed-warning observed: patch failed => FAIL"; continue; }
    echo "== MT-$m mt's table patched ($m): named, not believed, no crash"
    r MT-$m MASON_ODR_STATS=1 -- ./mhostf o:$W/mt-$m.so:g o:$P/h2.so: c:$P/h2.so
    ok=1; [[ $LAST == reports=0\ * ]] && [[ $LAST == *exit=0 ]] && [ "$(warned odr-hook-malformed-table)" -ge 1 ] && ok=0
    if [ $ok = 1 ] && [[ $LAST == *exit=139 ]]; then
      # the loader itself may fault on the module before any runtime runs: the foreign
      # host (no runtime anywhere) opening the module alone tells
      (cd "$W" && xrun -- ./host o:$W/mt-$m.so:g > /dev/null 2>&1); fst=$?
      if [ $fst = 139 ]; then
        echo "MATRIX $T $RT_FLAVOR MT-$m required=none,malformed-warning observed: $LAST => loader-faults (the foreign host opening mt-$m.so alone: exit $fst; no runtime code ran)"
        continue
      fi
    fi
    row MT-$m none,malformed-warning $ok "[malformed=$(stv malformed) warning: $(printf '%s\n' "$LASTOUT" | grep -o 'odr-hook-malformed-table.*' | head -1 | sed 's|/[^ ]*/||g')]"
  done
  # the runtime module's own table malformed: it checks nothing, says so
  python3 "$H/mt_patch.py" "$P/h2.so" "$W/h2own.so" count
  echo "== MT-own the runtime module's own table malformed (count)"
  r MT-own MASON_ODR_STATS=1 -- ./mhostf o:$W/mt.so:g o:$W/h2own.so: c:$W/h2own.so
  ok=1; [[ $LAST == reports=0\ * ]] && [[ $LAST == *exit=0 ]] && [ "$(warned odr-hook-malformed-table)" -ge 1 ] && ok=0
  row MT-own none,malformed-warning $ok
fi
if want VC; then
  # a copy relocation of a versioned name: libvc defines lvl@V1 (5) and lvl@@V2 (6) and refers
  # to lvl (V2) by GLOB_DAT; a non-PIE executable copies lvl@V2, so the reference binds the
  # copy, and the copy's source has two versions of the name: counted unknown, no report
  printf 'int l1 = 5;\nint l2 = 6;\n__asm__(".symver l1,lvl@V1");\n__asm__(".symver l2,lvl@@V2");\n' > "$W/vcd.c"
  printf 'extern int lvl;\nint get_lvl(void) { return lvl; }\n' > "$W/vcu.c"
  printf 'V1 { global: lvl; get_lvl; local: *; };\nV2 { global: lvl; } V1;\n' > "$W/vc.map"
  printf '\t.section .note.mason,"a",%%note\n\t.p2align 3\n\t.long 10\n\t.long 64\n\t.long 0x4d4f4401\n\t.asciz "MasonMeta"\n\t.p2align 3\n\t.long 1, 2, 24, 0\n\t.zero 48\n\t.section .note.GNU-stack,"",%%progbits\n' > "$W/vcn.s"
  $CC -O1 -fPIC -c "$W/vcd.c" -o "$W/vcd.o" && $CC -O0 -fPIC -c "$W/vcu.c" -o "$W/vcu.o" && $CC -c "$W/vcn.s" -o "$W/vcn.o"
  $CC -shared "$W/vcd.o" "$W/vcu.o" "$W/vcn.o" "$RT" "$W/note-report.o" -Wl,-z,now -Wl,--version-script="$W/vc.map" -Wl,-soname,libvc.so -o "$W/libvc0.so"
  printf 'row name=lvl version=V1 hash=%s flags=1\nrow name=lvl version=V2 hash=%s flags=1\n' "$(hash_of 5 | cut -c1-16)" "$(hash_of 6 | cut -c1-16)" > "$W/vc.rows"
  "$XTASK" meta table "$W/vc.rows" --into "$W/libvc0.so" -o "$W/libvc.so"
  printf '#include <stdio.h>\nextern int lvl;\nint get_lvl(void);\nint main(void) { lvl = 9; printf("exe %%d lib %%d\\n", lvl, get_lvl()); return get_lvl() != 9; }\n' > "$W/vcm.c"
  $CC -O0 -fno-pic -c "$W/vcm.c" -o "$W/vcm.o" && $CC -no-pie "$W/vcm.o" -Wl,--no-as-needed -L"$W" -lvc -Wl,-rpath,'$ORIGIN' -o "$W/vcmain"
  echo "== VC libvc's GLOB_DAT to lvl@@V2 binds the executable's copy ($(readelf -rW "$W/vcmain" | grep -o 'R_[A-Z0-9_]*COPY.*' | tr -s ' ' | head -1))"
  r VC MASON_ODR_STATS=1 -- ./vcmain
  ok=1; [[ $LAST == reports=0\ other-warnings=0\ exit=0 ]] && [ "$(stv unknown)" -ge 1 ] && ok=0
  row VC none,counted-unknown $ok "[copy=$(stv copy) unknown=$(stv unknown)]"
fi
if want RB; then
  # WP-196's attacks on the runtime; each a row
  # RB-J2: J with 32 threads (a module another thread closes unloads during each entry's sweep)
  if [ ! -f "$W/thr/libj4.so" ]; then
    mkdir -p "$W/thr"
    for i in 1 2 3 4; do
      python3 "$H/gen_pool.py" "$W/thr/j$i" 10000 "j$i"
      $CC -O1 -fPIC -c "$W/thr/j$i.c" -o "$W/thr/j$i.0.o" && meta_file "$W/thr/j$i.meta" "$W/thr/j$i.0.o" "$W/thr/j$i.o"
      mlink $CC -shared "$W/thr/j$i.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -o "$W/thr/libj$i.so"
    done
  fi
  $CC -O2 "$H/threads.c" -o "$W/thr/st" -ldl -lpthread
  for k in 1 2; do
    s=$(date +%s%N)
    out=$(cd "$W" && timeout 900 "${RUN[@]}" ./thr/st "$W/thr" 32 100 2>&1); st=$?; out=$(echo "$out" | tr '\n' ' ')
    echo "MATRIX $T $RT_FLAVOR RB-J2-run$k required=no-crash,no-hang,no-report observed: 32 threads x 100, exit=$st [$out] $(( ($(date +%s%N) - s) / 1000000 )) ms => $([ "$out" = "done " ] && echo pass || echo FAIL)"
  done
  # RB-sym: the runtime module itself -Bsymbolic, or its definition protected: it binds its own
  plug h2s 2 rt -Wl,-Bsymbolic
  printf 'namespace fmtlite { __attribute__((visibility("protected"))) inline int version() { return 2; } }\nextern "C" int plugin_entry() { return fmtlite::version(); }\n' > "$W/h2p.cpp"
  $CXX -O0 -fPIC -c "$W/h2p.cpp" -o "$W/h2p.0.o" && meta "$W/h2p.0.o" "$W/h2p.o" "_ZN7fmtlite7versionEv=2"
  mlink $CXX -shared "$W/h2p.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -o "$W/h2p.so"
  for v in h2s h2p; do
    echo "== RB-sym-$v h1 GLOBAL, then $v LOCAL (-Bsymbolic / protected): its references stay its own"
    r RB-sym-$v MASON_ODR_STATS=1 -- ./mhostf o:$P/h1.so:g o:$W/$v.so: c:$W/$v.so
    ok=1; [[ $LAST == reports=0\ other-warnings=0\ exit=0 ]] && printf '%s\n' "$LASTOUT" | grep -q "$v.so -> 2" && ok=0
    row RB-sym-$v none $ok "[rows=$(stv rows) relocs=$(stv relocs) own=$(stv own)]"
  done
  # RB-relro: reading must not write: -z norelro, and -z relro -z now (the GOT read-only when the entry runs)
  for z in norelro relro; do
    plug h1$z 1 rt -Wl,-z,$z; plug h2$z 2 rt -Wl,-z,$z
    echo "== RB-$z h1$z GLOBAL, h2$z LOCAL binds h1$z's: report, no fault ($(readelf -lW "$W/h2$z.so" | grep -c GNU_RELRO) PT_GNU_RELRO)"
    r RB-$z -- ./mhost o:$W/h1$z.so:g o:$W/h2$z.so: c:$W/h2$z.so; verdict RB-$z report
  done
  if command -v prelink > /dev/null; then echo "RB-prelink: prelink is installed but not exercised here"
  else echo "RB-prelink: prelink is not installed on this host (unexercised)"; fi
  # RB-0rows: a runtime module whose table has no row; RB-norel: rows but no keyed relocation
  printf 'extern "C" int plugin_entry() { return 0; }\n' > "$W/z0.cpp"
  $CXX -O0 -fPIC -c "$W/z0.cpp" -o "$W/z0.0.o" && meta "$W/z0.0.o" "$W/z0.o" ""
  mlink $CXX -shared "$W/z0.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -o "$W/z0.so"
  plug zn 2 rt -Wl,-Bsymbolic-functions
  echo "== RB-0rows a runtime module with a table of no rows (note: $(readelf -nW "$W/z0.so" | grep -c 0x4d4f4401) table note(s))"
  r RB-0rows MASON_ODR_STATS=1 -- ./mhostf o:$W/z0.so: c:$W/z0.so
  ok=1; [[ $LAST == reports=0\ other-warnings=0\ exit=0 ]] && ok=0
  row RB-0rows none $ok "[rows=$(stv rows) relocs=$(stv relocs)]"
  echo "== RB-norel zn (-Bsymbolic-functions): a row and no keyed relocation"
  r RB-norel MASON_ODR_STATS=1 -- ./mhostf o:$P/h1.so:g o:$W/zn.so: c:$W/zn.so
  ok=1; [[ $LAST == reports=0\ other-warnings=0\ exit=0 ]] && ok=0
  row RB-norel none $ok "[rows=$(stv rows) relocs=$(stv relocs)]"
  # RB-relr: DT_RELR (-z pack-relative-relocs) in the runtime module and in the module it
  # binds; mortar refuses the option by name, so GNU ld links them (the test producer's table)
  if [ "$PRODUCER" = mortar ]; then
    plug h2relrm 2 rt -Wl,-z,pack-relative-relocs > "$W/relr-mortar.txt" 2>&1
    echo "RB-relr: mortar on -z pack-relative-relocs: $(grep -o 'unsupported option: .*' "$W/relr-mortar.txt" | head -1)"
  fi
  if PRODUCER=gnu plug h1relr 1 rt -Wl,-z,pack-relative-relocs && PRODUCER=gnu plug h2relr 2 rt -Wl,-z,pack-relative-relocs; then
    echo "== RB-relr h1relr GLOBAL, h2relr LOCAL binds h1relr's: report ($(readelf -dW "$W/h2relr.so" | grep -c '(RELR)') DT_RELR)"
    r RB-relr -- ./mhost o:$W/h1relr.so:g o:$W/h2relr.so: c:$W/h2relr.so; verdict RB-relr report
  else echo "MATRIX $T $RT_FLAVOR RB-relr required=report observed: the link refused -z pack-relative-relocs => FAIL"; fi
  # RB-pltrel: DT_PLTREL names the other format, in the runtime module (x) and in the module it binds (y)
  python3 "$H/mt_patch.py" "$P/h2.so" "$W/h2pl.so" pltrel
  python3 "$H/mt_patch.py" "$P/t1.so" "$W/t1pl.so" pltrel
  echo "== RB-pltrel-x h2's DT_PLTREL flipped"
  r RB-pltrel-x MASON_ODR_STATS=1 -- ./mhostf o:$P/h1.so:g o:$W/h2pl.so: c:$W/h2pl.so
  ok=1; case $LAST in *exit=139*|*exit=135*|*exit=132*) ;; *) { printf '%s\n' "$LASTOUT" | grep -q 'dlopen.*h2pl\|Inconsistency detected by ld.so' || [ "$(warned odr-hook-malformed-table)" -ge 1 ]; } && [[ $LAST == reports=0\ * ]] && ok=0 ;; esac
  row RB-pltrel-x refused-or-named $ok "[$(printf '%s\n' "$LASTOUT" | grep -o 'dlopen.*\|odr-hook-malformed-table.*\|Inconsistency detected by ld.so.*' | head -1 | sed 's|/[^ ]*/||g')]"
  echo "== RB-pltrel-y t1's DT_PLTREL flipped, h2 binds it"
  r RB-pltrel-y MASON_ODR_STATS=1 -- ./mhostf o:$W/t1pl.so:g o:$P/h2.so: c:$P/h2.so
  ok=1; case $LAST in *exit=139*|*exit=135*|*exit=132*) ;; *) { printf '%s\n' "$LASTOUT" | grep -q 'dlopen.*t1pl\|Inconsistency detected by ld.so' || [ "$(warned odr-hook-malformed-table)" -ge 1 ]; } && [[ $LAST == reports=0\ * ]] && ok=0 ;; esac
  row RB-pltrel-y refused-or-named $ok "[$(printf '%s\n' "$LASTOUT" | grep -o 'dlopen.*\|odr-hook-malformed-table.*\|Inconsistency detected by ld.so.*' | head -1 | sed 's|/[^ ]*/||g')]"
  # RB-<n>: the map's boundary: the violating module has exactly n .dynsym entries
  # (4,096 the stack map's last size, 4,097 the first mmap'd, 16,385 the prototype's first)
  for n in 4096 4097 16385; do
    d=$W/pad-$n; mkdir -p "$d"
    p=$(( (n - 40) * 4 / 5 ))
    for i in 1 2; do
      bad=""; [ $i = 2 ] && bad=bad
      python3 "$H/gen_pool.py" "$d/m$i" $p "s${i}x" $bad
    done
    have=0
    for pass in 1 2; do
      for i in 1 2; do
        $CC -O1 -fPIC -c "$d/m$i.c" -o "$d/m$i.0.o" && meta_file "$d/m$i.meta" "$d/m$i.0.o" "$d/m$i.o" || break 2
        mlink $CC -shared "$d/m$i.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,now -Wl,-soname,libm$i.so -o "$d/libm$i.so" || break 2
      done
      have=$(readelf --dyn-syms -W "$d/libm2.so" | sed -n "s/.*'.dynsym' contains \([0-9]*\) entries.*/\1/p")
      [ "$pass" = 2 ] || [ "$have" = "$n" ] && break
      for k in $(seq 1 $((n - have))); do echo "__attribute__((visibility(\"default\"))) int mason_pad_$k = 0;" >> "$d/m2.c"; done
    done
    echo 'int main(void){ return 0; }' > "$d/m.c"
    $CC "$d/m.c" -Wl,--no-as-needed -L"$d" -lm1 -lm2 -Wl,-rpath,'$ORIGIN' -o "$d/many"
    for v in map nommap; do
      e=(MASON_ODR_STATS=1); [ $v = nommap ] && e+=(MASON_ODR_TEST_NOMMAP=1)
      echo "== RB-$n-$v libm2: $have .dynsym entries, mk7 differs from libm1's"
      r RB-$n-$v "${e[@]}" -- ./pad-$n/many
      ok=1; [ "$have" = "$n" ] && [[ $LAST == reports=1\ other-warnings=0\ exit=0 ]] && ok=0
      row RB-$n-$v report $ok "[dynsym=$have slow-path lines=$(printf '%s\n' "$LASTOUT" | grep -c 'path=slow')]"
    done
  done
  if [ "${BIG:-0}" = 1 ]; then
    # RB-1M: one module with a table of 1,000,000 rows (800,000 pooled + 200,000 private)
    many 1 800000 good big || echo "RB-1M: build failed"
    echo "== RB-1M a 1,000,000-row table"
    s=$(date +%s%N)
    r RB-1M MASON_ODR_STATS=1 MASON_ODR_TIME=1 -- ./many-big/many
    ok=1; [[ $LAST == reports=0\ other-warnings=0\ exit=0 ]] && [ "$(stv rows)" = 1000000 ] && ok=0
    row RB-1M none $ok "[rows=$(stv rows) relocs=$(stv relocs) own=$(stv own) check $(printf '%s\n' "$LASTOUT" | awk '/mason-time/ { printf "%.1f ms", $2 / 1e6 }'), run $(( ($(date +%s%N) - s) / 1000000 )) ms]"
  fi
fi
echo "run.sh: done $T $RT_FLAVOR"
