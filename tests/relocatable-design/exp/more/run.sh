#!/bin/bash
# more/run.sh WORK: the pre-link idiom (ld -r, objcopy --localize-hidden or -G,
# final link by GNU ld and by mortar, run); -gsplit-dwarf at -r; Fortran;
# -i; LTO objects at -r; .gdb_index on a -r line; -T script output final-linked.
set -u; ulimit -c 0
W=$(realpath -m "$1"); H=$(cd "$(dirname "$0")" && pwd); C=$H/../corpus
M=/Volumes/AgentWork/mortar-helper/.build/target-wp-154/mortar-3ad9867a
mkdir -p "$W/mbin" "$W/tmp"; cd "$W" || exit 1; export TMPDIR=$W/tmp; ln -sf "$M" mbin/ld
F="-O1 -g -fcommon -ffunction-sections -fdata-sections"
for s in c1 c2 c3 c4 mainc; do gcc $F -c "$C/$s.c" -o $s.o; done
for s in x1 x2 mainx; do g++ $F -c "$C/$s.cc" -o $s.o; done
gcc $F c1.o c2.o c4.o mainc.o -o direct.c 2>/dev/null; ./direct.c > want.c
g++ $F x1.o x2.o mainx.o -o direct.x 2>/dev/null; ./direct.x > want.x
fin() { # fin NAME LANG INPUTS...: final link by GNU ld and mortar, run, compare
  local n=$1 l=$2; shift 2; local drv=gcc; [ $l = x ] && drv=g++
  for L in bfd mortar; do
    local B=""; [ $L = mortar ] && B="-B mbin/"
    $drv -static $B -o $n.$L "$@" > $n.$L.log 2>&1 || { echo "$n | $L | link rc=$? $(grep -m1 error $n.$L.log | cut -c1-160)"; continue; }
    ./$n.$L > $n.$L.out 2>&1; echo "$n | $L | run rc=$? $(cmp -s $n.$L.out want.$l && echo same || echo DIFFERS)"
  done
}
# 1. pre-link idiom (c3.o, .ctors, is left out: mortar refuses .ctors in any link)
for K in bfd lld mold; do case $K in bfd) LD=ld.bfd;; lld) LD=ld.lld-22;; mold) LD=mold;; esac
  $LD -r -o comp.$K.o c1.o c2.o
  objcopy --localize-hidden comp.$K.o comp.$K.lh.o
  echo "prelink-$K | hid in comp: $(readelf -sW comp.$K.o | awk '$8=="hid"{print $5"/"$6}') after --localize-hidden: $(readelf -sW comp.$K.lh.o | awk '$8=="hid"{print $5"/"$6}')"
  fin prelink-lh-$K c comp.$K.lh.o c4.o mainc.o
  objcopy -G g_data -G wk -G common_x -G tv -G msg_get comp.$K.o comp.$K.G.o 2>/dev/null
  $LD -r -o xcomp.$K.o x1.o x2.o; objcopy --localize-hidden xcomp.$K.o xcomp.$K.lh.o
  fin prelink-lh-x-$K x xcomp.$K.lh.o mainx.o
done
# 2. -gsplit-dwarf objects at -r
for s in x1 x2; do g++ -O1 -gsplit-dwarf -c "$C/$s.cc" -o sd.$s.o; done
for K in bfd lld mold; do case $K in bfd) LD=ld.bfd;; lld) LD=ld.lld-22;; mold) LD=mold;; esac
  $LD -r -o sd.$K.o sd.x1.o sd.x2.o 2> sd.$K.err || { echo "splitdwarf-$K | -r failed $(head -1 sd.$K.err)"; continue; }
  echo "splitdwarf-$K | skeleton CUs: $(readelf --debug-dump=info sd.$K.o 2>/dev/null | grep -c DW_TAG_skeleton_unit) dwo names: $(readelf --debug-dump=info sd.$K.o 2>/dev/null | grep -o 'sd\.x[12]\.dwo' | sort -u | tr '\n' ' ')"
  fin splitdwarf-$K x sd.$K.o mainx.o
done
# 3. Fortran: a module with bind(c) and a module variable
gfortran -O1 -g -c "$H/f1.f90" -o f1.o; gcc -c "$H/fmain.c" -o fmain.o
printf '11\n' > want.f
for K in bfd lld mold; do case $K in bfd) LD=ld.bfd;; lld) LD=ld.lld-22;; mold) LD=mold;; esac
  $LD -r -o fr.$K.o f1.o
  for L in bfd mortar; do B=""; [ $L = mortar ] && B="-B mbin/"
    gfortran -static $B -o fort-$K.$L fmain.o fr.$K.o > fort-$K.$L.log 2>&1 || { echo "fortran-$K | $L | link rc=$? $(grep -m1 error fort-$K.$L.log | cut -c1-160)"; continue; }
    ./fort-$K.$L > fort.out; echo "fortran-$K | $L | $(cmp -s fort.out want.f && echo same || echo "DIFFERS: $(cat fort.out)")"
  done
done
# 4. -i (GNU ld's alias of -r)
ld.bfd -i -o i.o c1.o c2.o && ld.bfd -r -o r.o c1.o c2.o && echo "dash-i | bfd | $(cmp -s i.o r.o && echo 'identical to -r' || echo differs)"
ld.lld-22 -i -o i.lld.o c1.o c2.o > i.lld.log 2>&1; echo "dash-i | lld | rc=$? $(head -1 i.lld.log)"
mold -i -o i.mold.o c1.o c2.o > i.mold.log 2>&1; echo "dash-i | mold | rc=$? $(head -1 i.mold.log)"
# 5. LTO objects at -r
gcc -O1 -flto -c "$H/lto1.c" -o lto.gcc.o; clang-22 -O1 -flto -c "$H/lto1.c" -o lto.clang.o
for o in lto.gcc.o lto.clang.o; do
  ld.bfd -r -o r.$o $o > l.log 2>&1; echo "lto $o | bfd (no plugin) | rc=$? $(head -1 l.log | cut -c1-150) secs: $(readelf -SW r.$o 2>/dev/null | grep -c gnu.lto)"
  ld.lld-22 -r -o rl.$o $o > l.log 2>&1; echo "lto $o | lld | rc=$? $(head -1 l.log | cut -c1-150) type: $(file -b rl.$o 2>/dev/null | cut -c1-40)"
  mold -r -o rm.$o $o > l.log 2>&1; echo "lto $o | mold | rc=$? $(head -1 l.log | cut -c1-150)"
  $M -r -o rq.$o $o > l.log 2>&1; echo "lto $o | mortar | rc=$? $(head -1 l.log | cut -c1-150)"
done
# 6. --gdb-index on a -r line
ld.lld-22 -r --gdb-index -o gi.o x1.o x2.o > gi.log 2>&1; echo "gdb-index | lld | rc=$? $(head -1 gi.log | cut -c1-150)"
mold -r --gdb-index -o gim.o x1.o x2.o > gi.log 2>&1; echo "gdb-index | mold | rc=$? $(head -1 gi.log | cut -c1-150) $(readelf -SW gim.o 2>/dev/null | grep -c gdb_index)"
# 7. -T script with -r: merge .text.* into .text, then final link
printf 'SECTIONS { .text : { *(.text .text.*) } .data : { *(.data .data.*) } }\n' > merge.ld
for K in bfd lld; do case $K in bfd) LD=ld.bfd;; lld) LD=ld.lld-22;; esac
  $LD -r -T merge.ld -o ts.$K.o c1.o c2.o 2> ts.err || { echo "script-$K | -r failed"; continue; }
  echo "script-$K | .text.* left: $(readelf -SW ts.$K.o | grep -c ' \.text\.') rela.text records: $(readelf -rW ts.$K.o | awk '/Relocation section .\.rela\.text./{f=1;next} /Relocation section/{f=0} f&&/R_/' | wc -l)"
  fin script-$K c ts.$K.o c4.o mainc.o
done
echo "more done"
