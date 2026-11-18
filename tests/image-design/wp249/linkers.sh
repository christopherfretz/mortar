#!/bin/bash
# WP-249 step 1, item 2: what GNU ld, lld 19/22, mold and mortar write for
# GCC 14's DW_MACRO_import into a dropped .debug_macro copy, judged by
# macros.py; then gdb 16 (AArch64 natively; x86-64 by gdb-multiarch on
# qemu-x86_64 -g) and lldb 19 (AArch64 only: it runs host programs) on
# each output, and on mortar's output patched to the kept copy's offset
# ('redirect') and to 0 ('zero'), plus llvm-dwarfdump-22 --verify and
# readelf's warnings.
#   linkers.sh OUT [MORTAR]     OUT/judge.txt, OUT/debug.tsv; last line LINKERS DONE
# The links run in ONE fixed directory, emptied after each configuration
# (literal path: the workspace's rm rule).
ulimit -c 0
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$1"; MORTAR="${2:-/Volumes/AgentWork/mortar-helper/.build/target-wp-143/release/mortar}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
WORK=/Volumes/AgentWork/mortar-helper/.build/target-wp-143/wp249/lkwork
mkdir -p "$WORK/bm" "$WORK/l19" "$WORK/l22"; ln -sf "$MORTAR" "$WORK/bm/ld"
ln -sf /usr/bin/ld.lld-19 "$WORK/l19/ld.lld"; ln -sf /usr/bin/ld.lld-22 "$WORK/l22/ld.lld"
: > "$OUT/judge.txt"; : > "$OUT/debug.tsv"; mkdir -p "$OUT/sessions"
M=python3; MP="$HERE/macros.py"
freeport() { python3 -I -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()'; }
GDBCMDS=(-ex 'break b_f' -ex 'info macro __GNUC__' -ex 'info macro WP249_ANSWER'
         -ex 'macro expand WP249_TWICE(WP249_ANSWER)' -ex 'info macro WP249_A_ONLY' -ex 'bt')
gdb_a64() { timeout 120 gdb -nx -q -batch -ex 'set pagination off' -ex 'set debuginfod enabled off' \
  -ex 'break b_f' -ex run "${GDBCMDS[@]:2}" -ex "ptype $1" -ex kill "$2" 2>&1; }
gdb_x64() {
  local port; port=$(freeport)
  qemu-x86_64 -g "$port" "$2" > "$WORK/qemu.out" 2>&1 &
  local qp=$!
  sleep 1
  timeout 120 gdb-multiarch -nx -q -batch -ex 'set pagination off' -ex 'set debuginfod enabled off' \
    -ex "file $2" -ex "target remote 127.0.0.1:$port" -ex 'break b_f' -ex continue "${GDBCMDS[@]:2}" \
    -ex "ptype $1" -ex kill 2>&1
  kill $qp 2>/dev/null; wait $qp 2>/dev/null
}
lldb_a64() { timeout 120 lldb-19 --no-lldbinit -b -o 'settings set target.disable-aslr false' \
  -o 'b b_f' -o run -o bt -o "type lookup $1" -o 'expr WP249_TWICE(WP249_ANSWER)' \
  -o 'expr WP249_A_ONLY' -o kill "$2" 2>&1; }
# (lldb -b stops at the first command that fails: the expressions come last)
# what a session says, in one field each
gdb_says() { # file
  local f=$1
  if grep -q 'This is a bug\|Fatal signal\|internal-error' "$f"; then echo "CRASH"; return; fi
  local g a e o t
  g=$(grep -c -- '-D__GNUC__=14' "$f"); a=$(grep -c '^#define WP249_ANSWER 42' "$f")
  e=$(grep -c 'expands to: ((42) \* 2)' "$f"); o=$(grep -c 'The symbol `WP249_A_ONLY. has no definition' "$f")
  t=$(grep -c 'type = struct' "$f")
  echo "gnuc=$g answer=$a expand=$e a_only_undefined=$o ptype=$t bt=$(grep -c '^#1 ' "$f")"
}
lldb_says() {
  local f=$1
  if grep -q 'PLEASE submit a bug report\|Stack dump' "$f"; then echo "CRASH"; return; fi
  echo "expr84=$(grep -c '= 84$' "$f") a_only_undeclared=$(grep -c "use of undeclared identifier 'WP249_A_ONLY'" "$f") a_only_1=$(grep -c '^(int) \$1 = 1$' "$f") type=$(grep -c '^struct\|^class' "$f") bt=$(grep -c 'frame #1' "$f")"
}
for tgt in a64 x64; do
 case $tgt in a64) CC=gcc-14; CXX=g++-14;; x64) CC=x86_64-linux-gnu-gcc-14; CXX=x86_64-linux-gnu-g++-14;; esac
 for lang in c cxx; do
  case $lang in c) D=$CC; E=c; TY='struct wp249_point';; cxx) D=$CXX; E=cc; TY='Point';; esac
  for set in g3 g3O2 g3gz; do
   case $set in g3) F='-O0 -g3';; g3O2) F='-O2 -g3 -ffunction-sections -fdata-sections';; g3gz) F='-O0 -g3 -gz';; esac
   for v in 4 5; do
    cfg="gcc14-$tgt-$lang-$set-dw$v"
    find /Volumes/AgentWork/mortar-helper/.build/target-wp-143/wp249/lkwork -mindepth 1 -maxdepth 1 -type f -delete
    for f in a b c; do (cd "$WORK" && $D $F -gdwarf-$v -c "$HERE/src/$f.$E" -o $f.o) || echo "$cfg compile $f failed" >> "$OUT/judge.txt"; done
    L="-static $WORK/a.o $WORK/b.o $WORK/c.o"
    (cd "$WORK" && $D -B"$WORK/bm/" $L -o mortar) > "$WORK/mortar.err" 2>&1 || echo "$cfg mortar: $(head -2 "$WORK/mortar.err")" >> "$OUT/judge.txt"
    (cd "$WORK" && $D $L -o gnu) > /dev/null 2>&1
    (cd "$WORK" && $D -B"$WORK/l19/" -fuse-ld=lld $L -o lld19) > /dev/null 2>&1
    (cd "$WORK" && $D -B"$WORK/l22/" -fuse-ld=lld $L -o lld22) > /dev/null 2>&1
    (cd "$WORK" && $D -fuse-ld=mold $L -o mold) > /dev/null 2>&1
    outs="mortar gnu lld19 lld22 mold"
    if [ $set != g3gz ] && [ -f "$WORK/mortar" ]; then
      $M -I "$MP" patch "$WORK/mortar" "$WORK/redirect" redirect "$WORK/a.o" "$WORK/b.o" "$WORK/c.o" > /dev/null
      $M -I "$MP" patch "$WORK/mortar" "$WORK/zero" zero "$WORK/a.o" "$WORK/b.o" "$WORK/c.o" > /dev/null
      outs="$outs redirect zero"
    fi
    for x in $outs; do
      [ -f "$WORK/$x" ] || { echo "$cfg $x: no output" >> "$OUT/judge.txt"; continue; }
      $M -I "$MP" judge "$WORK/$x" "$cfg $x" "$WORK/a.o" "$WORK/b.o" "$WORK/c.o" >> "$OUT/judge.txt" 2>&1
      [ $set = g3 ] || continue
      ver=$(llvm-dwarfdump-22 --verify "$WORK/$x" 2>&1 | grep -c '^error:')
      rw=$(readelf --debug-dump=macro "$WORK/$x" 2>&1 >/dev/null | grep -ci 'warn')
      s="$OUT/sessions/$cfg.$x"
      gdb_$tgt "$TY" "$WORK/$x" > "$s.gdb"
      line="$cfg	$x	verify_errors=$ver	readelf_warnings=$rw	gdb: $(gdb_says "$s.gdb")"
      if [ $tgt = a64 ]; then lldb_a64 "$TY" "$WORK/$x" > "$s.lldb"; line="$line	lldb: $(lldb_says "$s.lldb")"; fi
      echo "$line" >> "$OUT/debug.tsv"
    done
   done
  done
 done
done
find /Volumes/AgentWork/mortar-helper/.build/target-wp-143/wp249/lkwork -mindepth 1 -maxdepth 1 -type f -delete
echo "LINKERS DONE"
