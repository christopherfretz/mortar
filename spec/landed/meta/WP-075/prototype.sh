#!/bin/bash
# The prototype of spec/odr.md O-24 (WP-075, scope item 1): T-O-24's shapes over shared
# objects made by GNU ld (or lld) from g++ objects that carry test-producer units
# (`cargo xtask meta write --into`), with O-50..O-52's table put in by o24.py (a prepared
# .note.mason placeholder linked in, its rows written after the link, when the .dynsym
# indices exist). For each shape: what the rule says, what o24.py reports (O-24 as
# written: --literal; and the amended identity: --amended), and what the program prints.
#
# usage: prototype.sh WORK TARGET [LINKER]   TARGET = aarch64 | x86_64; LINKER = bfd | lld
# XTASK must name the built xtask binary. Every program prints, per module, the value of
# fmtlite::version() that module's call returned (each module's own copy returns its N).
set -u
W=${1:?WORK}; T=${2:?TARGET}; LD=${3:-bfd}
HERE=$(cd "$(dirname "$0")" && pwd)
: "${XTASK:?XTASK=path/to/xtask}"
export XTASK
mkdir -p "$W"; cd "$W" || exit 1
ulimit -c 0
case $T in
  aarch64) CXX=aarch64-linux-gnu-g++; LDR=aarch64-linux-gnu-ld; RUN=() ;;
  x86_64) CXX=x86_64-linux-gnu-g++; LDR=x86_64-linux-gnu-ld; RUN=(qemu-x86_64 -L /usr/x86_64-linux-gnu) ;;
  *) echo "unknown target $T"; exit 2 ;;
esac
LDF=(-fuse-ld="$LD")
P="python3 -I $HERE/o24.py"
NAME=_ZN7fmtlite7versionEv

unit() { # unit OBJ N: a test-producer unit with fmtlite::version's kind 2 row, body N
  local h; h=$(printf '%02x' "$2"); h=$h$h$h$h$h$h$h$h; h=$h$h
  printf 'summary name=source value=%s\ndefinition anchor=%s hash=%s class=1 flags=1 file=fmtlite.h line=1\n' \
    "${1%.o}.cpp" "$NAME" "$h" > "${1%.o}.desc"
  # the unit as assembly, joined to the compiled object by `ld -r` (MT-5 keeps it)
  "$XTASK" meta write "${1%.o}.desc" -o "${1%.o}.unit.s" || exit 1
  $CXX -c "${1%.o}.unit.s" -o "${1%.o}.unit.o" && $LDR -r "$1.plain" "${1%.o}.unit.o" -o "$1" || exit 1
}

lib() { # lib NAME N VIS [link flags...]; VIS = default | protected | hidden
  local n=$1 v=$2 vis=$3; shift 3
  local attr=""; [ "$vis" != default ] && attr="__attribute__((visibility(\"$vis\")))"
  printf 'namespace fmtlite { %s inline int version() { return %d; } }\nextern "C" int %s_entry() { return fmtlite::version(); }\n' \
    "$attr" "$v" "$n" > "$n.cpp"
  [ -n "${LIBEXTRA:-}" ] && printf '%s\n' "$LIBEXTRA" >> "$n.cpp"
  $CXX -O0 -fPIC -c "$n.cpp" -o "$n.o.plain" || exit 1
  unit "$n.o" "$v"
  local bsf=""; for f in "$@"; do [ "$f" = -Wl,-Bsymbolic-functions ] && bsf=--bsf; done
  # pass 1: the row count; pass 2: the placeholder, then the rows
  $CXX "${LDF[@]}" -shared "$n.o" -o "lib$n.so" -Wl,-soname,"lib$n.so" "$@" || exit 1
  local c; c=$($P rows "lib$n.so" $bsf "$n.o")
  $P note "$c" > "$n.note.s"; $CXX -c "$n.note.s" -o "$n.note.o"
  $CXX "${LDF[@]}" -shared "$n.o" "$n.note.o" -o "lib$n.so" -Wl,-soname,"lib$n.so" "$@" || exit 1
  $P patch "lib$n.so" $bsf "$n.o" || exit 1
}

exe() { # exe OUT "ENTRIES" EXE_N EXE_VIS [link args...]; EXE_N = - for no own copy
  local out=$1 entries=$2 en=$3 evis=$4; shift 4
  {
    echo '#include <cstdio>'
    for e in $entries; do echo "extern \"C\" int ${e}_entry();"; done
    if [ "$en" != - ]; then
      local attr=""; [ "$evis" != default ] && attr="__attribute__((visibility(\"$evis\")))"
      echo "namespace fmtlite { $attr inline int version() { return $en; } }"
    fi
    echo 'int main() {'
    [ "$en" != - ] && echo '  std::printf("exe=%d ", fmtlite::version());'
    for e in $entries; do echo "  std::printf(\"$e=%d \", ${e}_entry());"; done
    echo '  std::printf("\n"); return 0; }'
  } > "$out.cpp"
  $CXX -O0 -c "$out.cpp" -o "$out.o.plain" || exit 1
  if [ "$en" != - ]; then unit "$out.o" "$en"; else cp "$out.o.plain" "$out.o"; fi
  $CXX "${LDF[@]}" "$out.o" -o "$out" -L. -Wl,-rpath-link,. "$@" || exit 1
}

run() { # run EXE: the program's own line
  (export LD_LIBRARY_PATH=$PWD; "${RUN[@]}" ./"$1" 2>&1 | head -3; echo "  (exit ${PIPESTATUS[0]})")
}

shape() { # shape ID "rule" EXE OBJ -- LIBS...
  local id=$1 rule=$2 x=$3; shift 3
  echo "== $id"
  echo "  rule:     $rule"
  echo "  literal:"; $P check "$x" --literal "$@" | sed 's/^/  /'
  echo "  amended:"; $P check "$x" --amended "$@" | sed 's/^/  /'
  echo -n "  ran:      "; run "$x"
}

# The libraries (N = the body each module's own copy returns).
lib a 1 default
lib b 2 default
lib c3 3 default
lib sym 1 default -Wl,-Bsymbolic
lib bsf 1 default -Wl,-Bsymbolic-functions
lib prot 1 protected
lib hid 1 hidden
printf 'LIB1_1 { global: *; };\n' > v1.map; printf 'LIB2_1 { global: *; };\n' > v2.map
printf 'V1 { global: *; };\n' > same.map
lib v1 1 default -Wl,--version-script=v1.map
lib v2 2 default -Wl,--version-script=v2.map
lib s1 1 default -Wl,--version-script=same.map
lib s2 2 default -Wl,--version-script=same.map
# a library whose own DT_NEEDED brings libd (body 3); d_entry is called through a_entry2
printf 'namespace fmtlite { inline int version() { return 3; } }\nextern "C" int d_entry() { return fmtlite::version(); }\n' > d.cpp
$CXX -O0 -fPIC -c d.cpp -o d.o.plain; unit d.o 3
c=$($CXX "${LDF[@]}" -shared d.o -o libd.so -Wl,-soname,libd.so && $P rows libd.so d.o)
$P note "$c" > d.note.s; $CXX -c d.note.s -o d.note.o
$CXX "${LDF[@]}" -shared d.o d.note.o -o libd.so -Wl,-soname,libd.so; $P patch libd.so d.o
printf 'namespace fmtlite { inline int version() { return 1; } }\nextern "C" int d_entry();\nextern "C" int e_entry() { return fmtlite::version(); }\nextern "C" int ed_entry() { return d_entry(); }\n' > e.cpp
$CXX -O0 -fPIC -c e.cpp -o e.o.plain; unit e.o 1
c=$($CXX "${LDF[@]}" -shared e.o -o libe.so -Wl,-soname,libe.so -L. -ld && $P rows libe.so e.o)
$P note "$c" > e.note.s; $CXX -c e.note.s -o e.note.o
$CXX "${LDF[@]}" -shared e.o e.note.o -o libe.so -Wl,-soname,libe.so -L. -ld; $P patch libe.so e.o

echo "# target $T, linker $LD; tables:"
for l in liba.so libsym.so libbsf.so libprot.so libhid.so libv1.so libs1.so libd.so; do
  echo "  $l: $($P table $l | tr '\n' ';')"
done

exe a1 "a" 2 default -la
shape "(a) exported: exe body 2 exports version, liba body 1" "finding (a)" a1 a1.o -- liba.so
exe a2 "a" 2 hidden -la
shape "(a) hidden: exe body 2 hidden, liba body 1" "none (not exported)" a2 a2.o -- liba.so
exe a3 "a b" 2 hidden -la -lb
shape "(a)+(b) exe body 2 hidden; liba 1, libb 2" "(b) only if 'does not define' means 'does not export'" a3 a3.o -- liba.so libb.so
exe a4 "v1" 2 default -lv1
shape "(a) exe body 2 exported unversioned; libv1 body 1 at LIB1_1" "none as written (different nodes)" a4 a4.o -- libv1.so
exe b1 "a b" - default -la -lb
shape "(b) liba 1, libb 2" "finding (b): liba provides to both" b1 b1.o -- liba.so libb.so
exe b1r "a b" - default -lb -la
shape "(b) libb 2, liba 1 (reversed)" "finding (b): libb provides to both" b1r b1r.o -- liba.so libb.so
exe b2 "sym b" - default -lsym -lb
shape "(b) libsym 1 -Bsymbolic, libb 2" "finding (R4/R5)" b2 b2.o -- libsym.so libb.so
exe b2r "sym b" - default -lb -lsym
shape "(b) libb 2, libsym 1 -Bsymbolic (reversed)" "finding (R4/R5)" b2r b2r.o -- libsym.so libb.so
exe b3 "bsf b" - default -lbsf -lb
shape "(b) libbsf 1 -Bsymbolic-functions, libb 2" "finding (R4/R5)" b3 b3.o -- libbsf.so libb.so
exe b4 "prot b" - default -lprot -lb
shape "(b) libprot 1 protected, libb 2" "finding (R4/R5)" b4 b4.o -- libprot.so libb.so
exe b4r "prot b" - default -lb -lprot
shape "(b) libb 2, libprot 1 protected (reversed)" "finding (R4/R5)" b4r b4r.o -- libprot.so libb.so
exe b5 "hid b" - default -lhid -lb
shape "(b) libhid 1 hidden (no row), libb 2" "none (hidden: no row)" b5 b5.o -- libhid.so libb.so
exe b6 "a" - default -la -Wl,--as-needed -lc3 -Wl,--no-as-needed
shape "(b) liba 1; libc3 3 dropped by --as-needed" "none (not in DT_NEEDED)" b6 b6.o -- liba.so libc3.so
exe b7 "v1 v2" - default -lv1 -lv2
shape "(b) libv1 1 at LIB1_1, libv2 2 at LIB2_1 (R6)" "none (different nodes)" b7 b7.o -- libv1.so libv2.so
exe b8 "s1 s2" - default -ls1 -ls2
shape "(b) libs1 1, libs2 2, both at node V1" "finding (one node name)" b8 b8.o -- libs1.so libs2.so
exe b9 "a v2" - default -la -lv2
shape "(b) liba 1 unversioned, libv2 2 at LIB2_1" "none as written (different nodes)" b9 b9.o -- liba.so libv2.so
exe b9r "a v2" - default -lv2 -la
shape "(b) libv2 2 at LIB2_1, liba 1 unversioned (reversed)" "none as written (different nodes)" b9r b9r.o -- liba.so libv2.so
exe b10 "e ed" - default -le
shape "(b) libe 1, whose DT_NEEDED brings libd 3 (closure)" "none (libd not read)" b10 b10.o -- libe.so libd.so

# ---- The version shapes of the ruling of 2026-11-15 (rows 4, 16, 17 refined) ----
# libnv: f@V1 (body 21, non-default, by .symver) and f@@V2 (body 22); nv_entry calls f.
cat > nv.cpp <<'C'
extern "C" int nv_old() { return 21; }
extern "C" int nv_new() { return 22; }
__asm__(".symver nv_old, _ZN7fmtlite7versionEv@V1");
__asm__(".symver nv_new, _ZN7fmtlite7versionEv@@V2");
namespace fmtlite { int version(); }
extern "C" int nv_entry() { return fmtlite::version(); }
C
printf 'V1 { local: nv_old; nv_new; };\nV2 { global: nv_entry; } V1;\n' > nv.map
$CXX -O0 -fPIC -c nv.cpp -o nv.o.plain || exit 1
printf 'definition anchor=%s@V1 hash=%s class=1 flags=1\ndefinition anchor=%s@@V2 hash=%s class=1 flags=1\n' \
  $NAME 15151515151515151515151515151515 $NAME 16161616161616161616161616161616 > nv.desc
"$XTASK" meta write nv.desc -o nv.unit.s && $CXX -c nv.unit.s -o nv.unit.o && $LDR -r nv.o.plain nv.unit.o -o nv.o || exit 1
c=$($CXX "${LDF[@]}" -shared nv.o -o libnv.so -Wl,-soname,libnv.so -Wl,--version-script=nv.map && $P rows libnv.so nv.o)
$P note "$c" > nv.note.s; $CXX -c nv.note.s -o nv.note.o
$CXX "${LDF[@]}" -shared nv.o nv.note.o -o libnv.so -Wl,-soname,libnv.so -Wl,--version-script=nv.map || exit 1
$P patch libnv.so nv.o || exit 1
# the old libnv (only f@@V1, body 21), to link libu against: libu's reference is f@V1
mkdir -p old
printf 'namespace fmtlite { int version() { return 21; } }\n' > nvold.cpp
printf 'V1 { global: *; };\n' > nvold.map
$CXX -O0 -fPIC -shared nvold.cpp -o old/libnv.so -Wl,-soname,libnv.so -Wl,--version-script=nvold.map "${LDF[@]}" || exit 1
printf 'namespace fmtlite { int version(); }\nextern "C" int u_entry() { return fmtlite::version(); }\n' > u.cpp
$CXX -O0 -fPIC -shared u.cpp -o libu.so -Wl,-soname,libu.so -Lold -lnv "${LDF[@]}" || exit 1
# unversioned providers of f, body 1: p1 has a .gnu.version table (it needs glibc: index 1,
# no version definitions); p2 has none (-nostdlib, no versioned dependency); p3 has version
# definitions (a version script naming p3_entry only), f left at index 1 = the base node.
LIBEXTRA='#include <cstdio>
extern "C" int p1_need() { return std::puts("p1"); }' lib p1 1 default
lib p2 1 default -nostdlib
printf 'PV { global: p3_entry; };\n' > p3.map
lib p3 1 default -Wl,--version-script=p3.map
echo "# version tables (versym of f; .gnu.version present; version definitions):"
for l in libp1.so libp2.so libp3.so libnv.so libu.so; do
  echo "  $l: $($P versym $l $NAME)"
done
echo "  libnv.so table: $($P table libnv.so | tr '\n' ';')"
for p in p1 p2 p3; do
  exe "v$p" "$p nv u" - default -l$p -lnv -lu
  shape "(versions) lib$p 1 unversioned, then libnv f@V1=21 f@@V2=22, then libu (refers f@V1)" "see the record" "v$p" "v$p.o" -- lib$p.so libnv.so libu.so
  exe "v${p}r" "$p nv u" - default -lnv -l$p -lu
  shape "(versions) libnv first, then lib$p 1 unversioned, then libu (refers f@V1)" "see the record" "v${p}r" "v${p}r.o" -- lib$p.so libnv.so libu.so
done

# ---- (c) K18b: a copy-relocated array whose size differs ----
printf 'int arr[8] = {1,2,3,4,5,6,7,8};\nextern "C" int k_last() { return arr[7]; }\n' > k.cpp
$CXX -O0 -fPIC -shared k.cpp -o libk.so -Wl,-soname,libk.so "${LDF[@]}" || exit 1
printf 'int arr[4] = {1,2,3,4};\nextern "C" int k_last() { return arr[3]; }\n' > kold.cpp
$CXX -O0 -fPIC -shared kold.cpp -o old/libk.so -Wl,-soname,libk.so "${LDF[@]}" || exit 1
printf '#include <cstdio>\nextern int arr[4];\nextern "C" int k_last();\nint main() { std::printf("arr[3]=%%d k_last=%%d\\n", arr[3], k_last()); return 0; }\n' > c.cpp
$CXX -O0 -fno-pic -c c.cpp -o c.o.plain || exit 1
printf 'declaration anchor=arr ty=0x1234 size=16 type_name=A4_i flags=2\n' > c.desc
"$XTASK" meta write c.desc -o c.unit.s && $CXX -c c.unit.s -o c.unit.o && $LDR -r c.o.plain c.unit.o -o c.o || exit 1
$CXX "${LDF[@]}" -no-pie c.o -o c1 -L. -lk || exit 1
shape "(c) K18b: exe declares int arr[4], links and runs against libk int arr[8]" "finding (c)" c1 c.o -- libk.so
$CXX "${LDF[@]}" -no-pie c.o -o c2 -Lold -lk || exit 1
echo "  copy size in c1: $(readelf -W --dyn-syms c1 | grep ' arr' | awk '{print $3}'), in c2: $(readelf -W --dyn-syms c2 | grep ' arr' | awk '{print $3}')"
shape "(c) K18b: exe linked against the old libk (int arr[4]), run with the new one (int arr[8])" "none at link time (X12: another build at run time)" c2 c.o -- old/libk.so
