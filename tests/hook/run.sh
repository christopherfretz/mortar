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
if want S; then
  # fatal at program start: nothing user-written runs before the stop
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
  echo "== H a module of 25,000 rows (above the 16,384-symbol stack map): mmap'd map, then mmap refused (slow path, no memory)"
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
echo "run.sh: done $T $RT_FLAVOR"
