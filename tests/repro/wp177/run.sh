#!/bin/bash
# The shapes WP-176 handed to WP-177 (spec/landed/resolve/WP-176.md, "For
# WP-177"). AArch64, natively. Not a lane test: the lane runs tests/link.
# Usage: run.sh WORKDIR MORTAR   (MORTAR: a mortar binary; WORKDIR: new, empty)
set -u; ulimit -c 0
H=$(cd "$(dirname "$0")" && pwd); W=$(realpath -m "$1"); M=$(realpath "$2")
mkdir -p "$W/mbin"; cd "$W" || exit 1; ln -sf "$M" mbin/ld
one() { # one NAME DRIVER ARGS...: link with bfd, lld, mold, mortar; run
  local n=$1 drv=$2; shift 2
  for L in bfd lld mold mortar; do
    local B="-fuse-ld=$L"; [ $L = mortar ] && B="-B mbin/"
    if $drv $B "$@" -o $n.$L > $n.$L.log 2>&1; then
      ./$n.$L > $n.$L.out 2>&1; echo "$n | $L | run $? | $(tr '\n' '|' < $n.$L.out | cut -c1-120)"
    else echo "$n | $L | link fails | $(grep -v collect2 $n.$L.log | head -2 | tr '\n' ' ' | cut -c1-200)"; fi
  done
}
# 1. case (a) from a LIVE CIE: the supervisor's cur1 (personality pointer
#    against a LOCAL DW.ref.__gxx_personality_v0 of a dropped group).
for f in p1 p2 pm; do g++ -O1 -g -c "$H/$f.cc" -o $f.o; done
ld.bfd -r -o r.o p1.o p2.o
objcopy --localize-hidden r.o r.lh.o
objcopy --keep-global-symbol=_Z4api1i --keep-global-symbol=_Z4api2i r.o r.kg.o
one cie-lh g++ pm.o r.lh.o
one cie-kg g++ pm.o r.kg.o
# 2. case (a) from a LIVE LSDA's type table (the silent 0: catch-all).
g++ -O1 -c "$H/q1.cc" -o q1.o; g++ -O1 -c "$H/qm.cc" -o qm.o
ld.bfd -r -o rq.o q1.o
objcopy --localize-symbol=DW.ref._ZTISt9exception rq.o rq.ti.o
one lsda-ti g++ qm.o rq.ti.o
one lsda-direct g++ qm.o q1.o
# 3. case (b) from a CIE that NO live FDE uses (its function collected):
#    a correct program refused by name (bfd and lld refuse the strong form).
gcc -c "$H/ka.s" -o ka.o; gcc -c "$H/kbg.s" -o kbg.o; gcc -c "$H/km2.c" -o km2.o
one cie-unused gcc -static -Wl,--gc-sections km2.o ka.o kbg.o
