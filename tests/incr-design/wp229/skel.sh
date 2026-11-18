#!/bin/bash
# skel.sh WORK MORTAR [a64|x64]: WP-229 item 4 and ruling 8. a64 (the
# default): native, the programs run under gdb and lldb. x64: objects by
# x86_64-linux-gnu-g++ and clang++-22 --target=x86_64-linux-gnu, linked by
# x86_64-linux-gnu-g++; each program runs under qemu-x86_64 -g PORT, and
# gdb-multiarch (`target remote`) and lldb (`gdb-remote`) attach to it.
# llvm-dwp runs under `timeout 60`: llvm-dwp 19 and 22 do not finish on
# GCC 14's DWARF 5 m.dwo (alone, no linker involved).
#
# Per compiler (g++, clang++-22) and DWARF version (4, 5), with
# -gsplit-dwarf, .dwo files present:
#   plain   m.o u.o (v2), no --incremental: keeps the GNU name index
#   full    m.o u.o (v2), --incremental=full: the index left out (IN-55 (A))
#   emul    m.o dead.o u.o (v2), --incremental=full, then neutral.py: the
#           update's picture of an edit of u.cc (v1 -> v2): v1's skeleton
#           contributions dead in place (IN-51 block form, IN-53 whole
#           contributions tombstoned, IN-54 aranges rewritten), v2's at
#           the section tails (linked last). dead.o is v1's u.o with its
#           defined symbols renamed (collected by --gc-sections).
#   emuldrop the same with the dead unit's addr_base dropped (a dead
#           .debug_addr table nobody names).
# Every link has --gc-sections. gdb and lldb transcripts (addresses, pids,
# paths masked) are compared: full vs plain (item 4), emul and emuldrop vs
# full (ruling 8); then again after llvm-dwp packs the .dwo files (the
# .dwo files moved away). llvm-dwarfdump --verify on each output. One
# summary line per comparison; the last line is `skel: done`.
set -u; ulimit -c 0
W="$1"; M="$2"; T="${3:-a64}"
D=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$W/bin"; ln -sf "$M" "$W/bin/ld"
cd "$W" || exit 1
cat > h.h <<'E'
#pragma once
inline int hinl(int x) { return x * 5 + 1; }
struct Pt { int x, y; int sum() const { return x + y; } };
E
cat > m.cc <<'E'
#include <cstdio>
#include "h.h"
int u_f(int);
int main(int argc, char **) {
  Pt p{argc, 2};
  int r = u_f(argc) + p.sum() + hinl(argc);
  std::printf("%d\n", r);
  return 0;
}
E
genu() { # version k of u.cc; UOnly's full definition lives only in u.cc
  k=$1
  { [ "$k" = 2 ] && echo "// edited"
    cat <<E
#include "h.h"
struct UOnly { int a; long b; $( [ "$k" = 2 ] && echo 'int z;' ) int get() const { return a + (int)b; } };
static UOnly uo{$k, 2};
int u_f(int c) {
  UOnly x{c, 3};
  int r = x.get() + uo.get();
  return r + hinl(c);
}
E
  } > u.cc; }
LINE=7   # 'int r = x.get() ...' in version 2
cat > g.cmd <<E
set pagination off
set confirm off
break u_f
break u.cc:$LINE
run
bt
info source
info line
list u_f
ptype UOnly
ptype Pt
print uo
continue
info locals
info functions u_f
info functions ^hin
info scope u_f
quit
E
cat > l.cmd <<E
settings set target.disable-aslr false
breakpoint set -n u_f
breakpoint set -f u.cc -l $LINE
run
bt
frame variable
continue
frame variable
image lookup -t UOnly
image lookup -t Pt
image lookup -n u_f
image lookup -r -n ^hin
source list -n u_f
target variable uo
breakpoint list
quit
E
norm() { sed -e 's/0x[0-9a-f]*/ADDR/g' -e 's/[Pp]rocess [0-9]*/process N/g' -e "s#$W/##g" \
  -e 's/\(plain\|full\|emuldrop\|emul\)\b/X/g' -e 's/\.text + [0-9]*/.text + OFF/' -e 's/\.data + [0-9]*/.data + OFF/' \
  -e 's/\.bss + [0-9]*/.bss + OFF/' -e 's/PT_LOAD\[[0-9]\]/PT_LOAD[N]/g' \
  -e '/^(UOnly) x = (a = 1, b = 3/!s/^(UOnly) x = .*/(UOnly) x = UNINITIALIZED/' \
  -e '/^x = {a = 1, b = 3/!s/^x = {.*/x = UNINITIALIZED/' | grep -v '^(lldb) quit'; }
tr_run() { # dir out tag
  if [ "$T" = a64 ]; then
    ( cd "$1" && gdb -batch -nx -x ../g.cmd ./"$2" 2>&1 | norm > "$2.$3.gdb"
      lldb -b -s ../l.cmd ./"$2" 2>&1 | norm > "$2.$3.lldb" )
    return
  fi
  ( cd "$1" || exit 1
    P=$((20000 + RANDOM % 30000))
    sed -e "s/^run$/target remote localhost:$P\ncontinue/" -e 's/^quit$/kill\nquit/' ../g.cmd > g.x.cmd
    timeout 120 qemu-x86_64 -g $P ./"$2" > /dev/null 2>&1 &
    timeout 120 gdb-multiarch -batch -nx -x g.x.cmd ./"$2" 2>&1 | sed '/QEMU: Terminated/d' | norm > "$2.$3.gdb"
    wait
    P=$((20000 + RANDOM % 30000))
    sed -e "s/^run$/gdb-remote localhost:$P\ncontinue/" -e 's/^quit$/kill\nquit/' ../l.cmd > l.x.cmd
    timeout 120 qemu-x86_64 -g $P ./"$2" > /dev/null 2>&1 &
    timeout 120 lldb -b -s l.x.cmd ./"$2" 2>&1 | sed '/QEMU: Terminated/d' | norm > "$2.$3.lldb"
    wait )
}
cmpx() { # dir a b tag
  for t in gdb lldb; do
    if cmp -s "$1/$2.$4.$t" "$1/$3.$4.$t"; then r=SAME; else r="DIFF $(diff "$1/$2.$4.$t" "$1/$3.$4.$t" | grep -c '^[<>]')"; fi
    echo "$1 $4 $t $3 vs $2: $r"
  done
}
for comp in g++ clang++-22; do for dw in 4 5; do
  d="$T-$comp-dwarf$dw"; mkdir -p "$d"; cp m.cc h.h "$d"/
  ( cd "$d" || exit 1
    F="-O0 -g -gdwarf-$dw -gsplit-dwarf -ffunction-sections -fdata-sections"
    drv=$comp
    if [ "$T" = x64 ]; then
      case $comp in g++) comp=x86_64-linux-gnu-g++;; *) comp="$comp --target=x86_64-linux-gnu";; esac
      drv=x86_64-linux-gnu-g++
    fi
    $comp $F -c m.cc -o m.o
    genu 1; $comp $F -c u.cc -o u.o
    OC=objcopy; NM=nm; [ "$T" = x64 ] && OC=x86_64-linux-gnu-objcopy && NM=x86_64-linux-gnu-nm
    args=$($NM --defined-only u.o | awk '$2 ~ /[TDBRVWutdbr]/ && $3 !~ /^\./ && $3 !~ /^\$/ {printf "--redefine-sym %s=o1_%s ", $3, $3}')
    $OC $args u.o dead.o
    genu 2; $comp $F -c u.cc -o u.o
    L="$drv -static -B$W/bin -Wl,--gc-sections"
    $L m.o u.o -o plain 2> plain.err || echo "$d: plain link FAILED $(head -1 plain.err)"
    $L -Wl,--incremental=full m.o u.o -o full 2> full.err || echo "$d: full link FAILED $(head -1 full.err)"
    $L -Wl,--incremental=full m.o dead.o u.o -o emul 2> emul.err || echo "$d: emul link FAILED $(head -1 emul.err)"
    cp emul emuldrop
    echo "$d: emul: $(python3 -I "$D/neutral.py" emul u.dwo 2>&1 | tail -1)"
    echo "$d: emuldrop: $(python3 -I "$D/neutral.py" emuldrop u.dwo --addr-base=drop 2>&1 | tail -1)"
    for x in plain full emul emuldrop; do
      v=$(llvm-dwarfdump-22 --verify $x 2>&1 | grep -c 'error:')
      rw=$(readelf --debug-dump=info,abbrev,addr,rawline,aranges,Ranges $x 2>&1 >/dev/null | grep -ci warn)
      echo "$d: $x size=$(stat -c %s $x) gnu-index-sections=$(readelf -SW $x | grep -c debug_gnu_pub) verify-errors=$v readelf-warnings=$rw"
      llvm-dwarfdump-22 --verify $x 2>&1 | grep 'error:' | sed 's/0x[0-9a-f]*/ADDR/g' | sort | uniq -c | head -3 | sed "s/^/  $d $x verify: /"
      readelf --debug-dump=info,abbrev,addr,rawline,aranges,Ranges $x 2>&1 >/dev/null | grep -i warn | sed 's/0x[0-9a-f]*/ADDR/g' | sort | uniq -c | head -3 | sed "s/^/  $d $x readelf: /"
    done
  )
  for x in plain full emul emuldrop; do tr_run "$d" $x dwo; done
  cmpx "$d" plain full dwo; cmpx "$d" full emul dwo; cmpx "$d" full emuldrop dwo
  # dwp: one package per output, the .dwo files moved away.
  ( cd "$d" && mkdir -p dwos && for x in plain full emul emuldrop; do
      timeout 60 llvm-dwp-22 -e $x -o $x.dwp 2> $x.dwp.err || echo "$d: llvm-dwp $x FAILED $(head -1 $x.dwp.err)"
    done && mv m.dwo u.dwo dwos/ )
  for x in plain full emul emuldrop; do tr_run "$d" $x dwp; done
  cmpx "$d" plain full dwp; cmpx "$d" full emul dwp; cmpx "$d" full emuldrop dwp
  cmpx "$d" full full dwo >/dev/null
  for t in gdb lldb; do
    if cmp -s "$d/full.dwo.$t" "$d/full.dwp.$t"; then r=SAME; else r="DIFF $(diff "$d/full.dwo.$t" "$d/full.dwp.$t" | grep -c '^[<>]')"; fi
    echo "$d dwp-vs-dwo $t full: $r"
  done
  ( cd "$d" && mv dwos/m.dwo dwos/u.dwo . && rmdir dwos )
done; done
echo "skel $T: done"
