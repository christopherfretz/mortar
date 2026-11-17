#!/bin/bash
# debug/run.sh WORK: item B.5's debugging oracle. The C++ component of ../corpus
# with -g (DWARF 5) at -O0, linked directly, and through -r by GNU ld, lld 22 and
# mold, the final link by GNU ld and by mortar; gdb and lldb each stop at a
# file:line in x1.cc and in x2.cc and print a backtrace. The transcripts,
# normalised (addresses dropped), must equal the direct link's.
set -u; ulimit -c 0
WORK=$(realpath -m "$1"); HERE=$(cd "$(dirname "$0")" && pwd); C=$HERE/../corpus
MORTAR=${2:-/Volumes/AgentWork/mortar-helper/.build/target-wp-154/mortar-3ad9867a}
mkdir -p "$WORK/mbin"; cd "$WORK" || exit 1; ln -sf "$MORTAR" mbin/ld
for s in x1 x2 mainx; do g++ -g -O0 -ffunction-sections -c "$C/$s.cc" -o $s.o; done
g++ -static mainx.o x1.o x2.o -o direct
ld.bfd -r -o r.bfd.o x1.o x2.o; ld.lld-22 -r -o r.lld.o x1.o x2.o; mold -r -o r.mold.o x1.o x2.o
for k in bfd lld mold; do g++ -static mainx.o r.$k.o -o p.$k.bfd; g++ -static -B$WORK/mbin/ mainx.o r.$k.o -o p.$k.mortar; done
g++ -static -B$WORK/mbin/ mainx.o x1.o x2.o -o direct.mortar
gdbrun() { gdb -nx -batch -ex 'break x1.cc:10' -ex 'break x2.cc:8' -ex run -ex bt -ex continue -ex bt -ex 'info line x2_value' "$1" 2>&1 | grep -E '^#|^Breakpoint|^Line' | sed -e 's/0x[0-9a-f]*//g' -e 's/ in / /'; }
lldbrun() { lldb-19 -b -o 'settings set target.disable-aslr false' -o 'b x1.cc:10' -o 'b x2.cc:8' -o run -o bt -o continue -o bt "$1" 2>&1 | grep -E 'frame #|Breakpoint [0-9]+:' | sed -e 's/0x[0-9a-f]*//g' -e 's/`/ /g' -e 's/ at / /' | awk -v b="$1" '{$1=$1; for(i=1;i<=NF;i++) if($i==b) $i="BIN"; print}'; }
gdbrun direct > g.direct; lldbrun direct > l.direct
echo "direct gdb: $(wc -l < g.direct) lines; lldb: $(wc -l < l.direct) lines"; head -4 g.direct; head -4 l.direct
for p in direct.mortar p.bfd.bfd p.bfd.mortar p.lld.bfd p.lld.mortar p.mold.bfd p.mold.mortar; do
  gdbrun $p > g.$p; lldbrun $p > l.$p
  echo "$p: gdb $(cmp -s g.direct g.$p && echo same || echo DIFFERS) lldb $(cmp -s l.direct l.$p && echo same || echo DIFFERS)"
done
