#!/bin/bash
# WP-249 step 1, item 3: the class under --gc-sections, at -r, and in
# mortar's incremental mode. GCC 14 -O0 -g3 -ffunction-sections, C.
# u.o comes FIRST: it keeps every shared macro group while all of its code
# is collected (its one function is unused), so a redirect under
# --gc-sections names a copy whose file has no live code.
#   modes.sh OUT a64|x64 [MORTAR]   OUT/modes-T.txt; last line MODES DONE
ulimit -c 0
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$1"; T="$2"; MORTAR="${3:-/Volumes/AgentWork/mortar-helper/.build/target-wp-143/release/mortar}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
WORK=/Volumes/AgentWork/mortar-helper/.build/target-wp-143/wp249/mdwork
mkdir -p "$WORK/bm" "$WORK/l19"; ln -sf "$MORTAR" "$WORK/bm/ld"; ln -sf /usr/bin/ld.lld-19 "$WORK/l19/ld.lld"
find /Volumes/AgentWork/mortar-helper/.build/target-wp-143/wp249/mdwork -mindepth 1 -maxdepth 1 -type f -delete
case $T in
  a64) CC=gcc-14; LDR=ld.bfd; EM=aarch64linux; GDB=gdb;;
  x64) CC=x86_64-linux-gnu-gcc-14; LDR=x86_64-linux-gnu-ld.bfd; EM=elf_x86_64; GDB=gdb-multiarch;;
esac
R="$OUT/modes-$T.txt"; : > "$R"
J() { python3 -I "$HERE/macros.py" judge "$@" >> "$R" 2>&1; }
printf '#include <stdio.h>\n#include "h.h"\nint u_unused(void) { printf("u"); return WP249_ANSWER; }\n' > "$WORK/u.c"
cd "$WORK" || exit 1
for f in u a b c; do
  src="$HERE/src/$f.c"; [ $f = u ] && src="$WORK/u.c"
  $CC -O0 -g3 -ffunction-sections -fdata-sections -I"$HERE/src" -c "$src" -o $f.o || echo "compile $f failed" >> "$R"
done
O="u.o a.o b.o c.o"
echo "== --gc-sections (u.o first: its code is collected)" >> "$R"
$CC -static -B"$WORK/bm/" -Wl,--gc-sections $O -o m-gc 2>> "$R"
$CC -static -Wl,--gc-sections $O -o g-gc 2>> "$R"
$CC -static -B"$WORK/l19/" -fuse-ld=lld -Wl,--gc-sections $O -o l-gc 2>> "$R"
$CC -static -fuse-ld=mold -Wl,--gc-sections $O -o o-gc 2>> "$R"
for x in m-gc g-gc l-gc o-gc; do J $x "$T $x" $O; done
echo "u_unused in each output's symbol table (0: collected):" >> "$R"
for x in m-gc g-gc l-gc o-gc; do echo "  $x $(nm $x 2>/dev/null | grep -c ' u_unused$')" >> "$R"; done
python3 -I "$HERE/macros.py" patch m-gc m-gc-redirect redirect $O >> "$R"
J m-gc-redirect "$T m-gc-redirect" $O
if [ $T = a64 ]; then
  for x in m-gc g-gc m-gc-redirect; do
    echo "gdb $x: $(timeout 120 gdb -nx -q -batch -ex 'break b_f' -ex run -ex 'info macro WP249_ANSWER' -ex 'info macro WP249_A_ONLY' ./$x 2>&1 \
      | grep -c '^#define WP249_ANSWER 42\|has no definition') of 2 right; crash: $(timeout 120 gdb -nx -q -batch -ex 'break b_f' -ex run -ex 'info macro WP249_ANSWER' ./$x 2>&1 | grep -c 'Fatal signal')" >> "$R"
  done
fi
echo "== -r (u.o b.o -> ub.o; then ub.o a.o c.o linked)" >> "$R"
"$MORTAR" -m $EM -r u.o b.o -o m-ub.o 2>> "$R"; echo "mortar -r rc=$?" >> "$R"
$LDR -m $EM -r u.o b.o -o g-ub.o 2>> "$R"; echo "GNU ld -r rc=$?" >> "$R"
for x in m g; do
  [ -f $x-ub.o ] || continue
  echo "$x-ub.o: groups $(python3 -I "$HERE/classes.py" groups $x-ub.o | grep -c wm4) wm4 (u.o $(python3 -I "$HERE/classes.py" groups u.o | grep -c wm4) + b.o $(python3 -I "$HERE/classes.py" groups b.o | grep -c wm4)); $(readelf -rW $x-ub.o | awk '/^Relocation section .\.rela\.debug_macro/{n++} END{print n+0}') .rela.debug_macro sections" >> "$R"
  python3 -I "$HERE/classes.py" link "$x-ub.o" $x-ub.o | sed "s/^/  $x-ub.o alone: /" >> "$R"
  $CC -static -B"$WORK/bm/" $x-ub.o a.o c.o -o m-from-$x-r 2>> "$R"
  $CC -static $x-ub.o a.o c.o -o g-from-$x-r 2>> "$R"
  J m-from-$x-r "$T mortar link of $x-ub.o a.o c.o" $x-ub.o a.o c.o
  J g-from-$x-r "$T GNU link of $x-ub.o a.o c.o" $x-ub.o a.o c.o
done
echo "== --incremental (full link, then an update after touching b.o)" >> "$R"
$CC -static -B"$WORK/bm/" -Wl,--incremental $O -o m-inc 2>> "$R"; echo "full rc=$?" >> "$R"
J m-inc "$T m-inc full" $O
touch b.o
$CC -static -B"$WORK/bm/" -Wl,--incremental $O -o m-inc 2>> "$R"; echo "update rc=$?" >> "$R"
J m-inc "$T m-inc update" $O
find /Volumes/AgentWork/mortar-helper/.build/target-wp-143/wp249/mdwork -mindepth 1 -maxdepth 1 -type f -delete
echo "MODES DONE" >> "$R"
echo "MODES DONE"
