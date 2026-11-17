#!/bin/bash
# more/prelink.sh WORK: the pre-link idiom with C++ (EH, -g): which hidden
# symbols are group signatures or group members; ld -r plain vs
# --force-group-allocation, then objcopy --localize-hidden, final link by GNU
# ld and by mortar with mainx.o, run, gdb breakpoints and backtrace.
set -u; ulimit -c 0
W=$(realpath -m "$1"); H=$(cd "$(dirname "$0")" && pwd); C=$H/../corpus
M=/Volumes/AgentWork/mortar-helper/.build/target-wp-154/mortar-3ad9867a
mkdir -p "$W/mbin"; cd "$W" || exit 1; ln -sf "$M" mbin/ld
F="-O0 -g -ffunction-sections -fdata-sections"
for s in x1 x2 mainx; do g++ $F -c "$C/$s.cc" -o $s.o; done
g++ -static -o direct x1.o x2.o mainx.o; ./direct > want
gdbrun() { gdb -nx -batch -ex 'break x1.cc:10' -ex 'break x2.cc:8' -ex run -ex bt -ex continue -ex bt "$1" 2>&1 | grep -E '^#|^Breakpoint' | sed -e 's/0x[0-9a-f]*//g' -e 's/ in / /'; }
gdbrun direct > g.direct
hidden_in_groups() { python3 -I - "$1" <<'PY'
import sys; sys.path.insert(0, "/Volumes/AgentWork/mortar-helper/.build/target-wp-154/exp/tools")
from relf import *
e = load(sys.argv[1]); mem = {}; sig = set()
for g, flag, s, ms in e.groups():
    sig.add(s)
    for m in ms: mem[m] = s
n = {"sig": 0, "member": 0, "other": 0}
for y in e.syms[1:]:
    if y.vis == 2 and y.bind != 0 and y.shndx not in (0, 0xfff1, 0xfff2):
        n["sig" if y.name in sig else "member" if y.shndx in mem else "other"] += 1
print("hidden globals: group signatures %(sig)d, other group members %(member)d, outside groups %(other)d" % n, "; groups", len(e.groups()))
PY
}
for K in bfd bfd-fga lld lld-fga mold; do
  case $K in bfd) c="ld.bfd -r";; bfd-fga) c="ld.bfd -r --force-group-allocation";; lld) c="ld.lld-22 -r";; lld-fga) c="ld.lld-22 -r --force-group-allocation";; mold) c="mold -r";; esac
  $c -o comp.$K.o x1.o x2.o 2> e.log || { echo "$K | -r failed: $(head -1 e.log)"; continue; }
  objcopy --localize-hidden comp.$K.o comp.$K.lh.o
  echo "$K | -r: $(hidden_in_groups comp.$K.o)"
  for L in bfd mortar; do B=""; [ $L = mortar ] && B="-B mbin/"
    g++ -static $B -o p.$K.$L comp.$K.lh.o mainx.o > l.log 2>&1 || { echo "$K | $L | link failed: $(grep -m1 error l.log)"; continue; }
    ./p.$K.$L > out 2>&1; rc=$?; gdbrun p.$K.$L > g.$K.$L
    echo "$K | $L | rc=$rc run $(cmp -s out want && echo same || echo DIFFERS) gdb $(cmp -s g.direct g.$K.$L && echo same || echo DIFFERS)"
  done
done
objcopy --version | head -1
echo "prelink done"
