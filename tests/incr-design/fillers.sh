#!/bin/bash
# fillers.sh SCRATCH a64|x64|i386: IN-49a's filler form (WP-207: st_shndx 1,
# st_value = section 1's sh_addr) against every tool. Adapted from the R-L
# reviewer's r7-fillers.sh. X is an incremental-mode full link by mortar;
# Y0 is X with .symtab rebuilt by symfill.py WITHOUT fillers (a global
# removed, its entry taken by the last, the table moved to the file's end
# with slack); Y is the same WITH fillers. A tool reads the fillers as
# absent when its output on Y equals its output on Y0 (file names
# normalized). Also counts, in mortar's own outputs (plain and
# incremental-mode), the .symtab entries that could be confused with a
# filler: STT_SECTION entries and entries with st_name 0 other than entry 0.
# MORTAR: the prebuilt mortar binary (no cargo).
set -u
S=$1; T=$2; HERE=$(cd "$(dirname "$0")" && pwd)
MORTAR=${MORTAR:-/Volumes/AgentWork/mortar-helper/.build/target-wp-116/debug/mortar}
mkdir -p "$S/bin" "$S/fill-$T"; ln -sf "$MORTAR" "$S/bin/ld"; cd "$S/fill-$T" || exit 1
ulimit -c 0
case $T in
  a64) CXX=g++; P=; RUN=; GDB=gdb ;;
  x64) CXX=x86_64-linux-gnu-g++; P=x86_64-linux-gnu-; RUN=qemu-x86_64; GDB=gdb-multiarch ;;
  i386) CXX=i686-linux-gnu-g++; P=i686-linux-gnu-; RUN=qemu-i386; GDB=gdb-multiarch ;;
esac
cat > p.cc <<'X'
#include <cstdio>
#include <stdexcept>
static int counter;
int unused_global(int x) { return x * 7; }
__attribute__((noinline)) int helper(int x) { if (x > 100) throw std::runtime_error("big"); counter += x; return counter; }
__attribute__((noinline)) int middle(int x) { return helper(x) + 1; }
int main(int argc, char **) { int r = middle(argc); try { middle(1000); } catch (const std::exception &e) { std::printf("caught %s\n", e.what()); } std::printf("r=%d\n", r); return r; }
X
$CXX -g -O1 -c p.cc || exit 1
$CXX -B"$S/bin/" -static -g -o X p.o -Wl,--incremental=full > X.log 2>&1; echo "link X rc=$?"
$CXX -B"$S/bin/" -static -g -o Xp p.o > Xp.log 2>&1; echo "link Xp (plain) rc=$?"
for f in X Xp; do
  python3 -I - "$f" "${P}readelf" <<'PY'
import sys, subprocess
f, re_ = sys.argv[1], sys.argv[2]
out = subprocess.run([re_, '-sW', f], capture_output=True, text=True).stdout.splitlines()
rows = [l.split() for l in out if l.strip() and l.split()[0].rstrip(':').isdigit()]
sec = [r for r in rows if len(r) > 3 and r[3] == 'SECTION']
noname = [r for r in rows if len(r) == 7 and r[0] != '0:']
print(f'{f}: .symtab entries {len(rows)}, STT_SECTION {len(sec)}, unnamed (not entry 0) {len(noname)}')
PY
done
python3 -I "$HERE/symfill.py" X Y0 _Z13unused_globali nofill
python3 -I "$HERE/symfill.py" X Y _Z13unused_globali fill
chmod 755 Y Y0
echo "section 1: $(${P}readelf -SW Y | grep -F '[ 1]')"
t() { local n=$1; shift; "$@" > "$n.out" 2> "$n.err"; echo $? > "$n.rc"; }
for f in Y0 Y; do
  t $f.run $RUN ./$f
  t $f.readelf ${P}readelf -sW $f; t $f.readelf-a ${P}readelf -aW $f
  t $f.nm ${P}nm $f; t $f.nm-n ${P}nm -n $f; t $f.nm-S ${P}nm -S --size-sort $f; t $f.nm-a ${P}nm -a $f; t $f.nm-C ${P}nm -C -l $f
  t $f.llvm-nm llvm-nm $f; t $f.llvm-nm-a llvm-nm -a $f
  t $f.objdump-t ${P}objdump -t $f; t $f.objdump-d ${P}objdump -d $f
  t $f.llvm-objdump-d llvm-objdump -d $f; t $f.llvm-objdump-t llvm-objdump -t $f
  t $f.llvm-readelf llvm-readelf -sW $f; t $f.eu-readelf eu-readelf -s $f
  t $f.elflint eu-elflint --gnu-ld $f
  a=$(${P}nm $f | awk '$3=="_Z6helperi"{print $1}')
  t $f.addr2line ${P}addr2line -f -C -e $f 0x$a
  t $f.symbolizer llvm-symbolizer --obj=$f 0x$a
  t $f.gdb $GDB -batch -nx -ex 'info functions ^(helper|middle|main)' -ex 'info symbol _Z6helperi' -ex 'info line helper' -ex 'break helper' $f
  t $f.lldb lldb -b -o 'image lookup -n helper' -o 'image lookup -s middle' -o 'breakpoint set -n helper' $f
  t $f.strip ${P}strip -o $f.strip $f; t $f.strip-g ${P}strip -g -o $f.stripg $f
  t $f.objcopy ${P}objcopy $f $f.objcopy; t $f.only-keep-debug ${P}objcopy --only-keep-debug $f $f.dbg
  t $f.llvm-strip llvm-strip -o $f.lstrip $f; t $f.llvm-objcopy llvm-objcopy $f $f.lobjcopy
  for g in strip stripg objcopy lstrip lobjcopy; do t $f.$g.run $RUN ./$f.$g; done
  t $f.stripg.nm ${P}nm $f.stripg; t $f.objcopy.nm ${P}nm $f.objcopy; t $f.lobjcopy.nm ${P}nm $f.lobjcopy
  t $f.objcopy.readelf ${P}readelf -sW $f.objcopy
  if [ $T = a64 ]; then
    t $f.gdb-run gdb -batch -nx -ex 'break helper' -ex 'run' -ex 'bt' -ex 'info symbol $pc' -ex 'p counter' $f
    t $f.lldb-run lldb -b -o 'breakpoint set -n helper' -o run -o bt $f
  fi
done
# compare: same rc, same stdout and stderr once the file name is normalized
# and lldb's/gdb's process ids and addresses of the run are dropped
for o in Y.*.rc; do
  n=${o#Y.}; n=${n%.rc}
  norm() { sed -e 's/\bY0\b/F/g; s/\bY\b/F/g; s/process [0-9]*/process N/g; s/pid = [0-9]*/pid N/g' "$1"; }
  if [ "$(cat Y.$n.rc)" = "$(cat Y0.$n.rc)" ] && diff -q <(norm Y.$n.out) <(norm Y0.$n.out) > /dev/null && diff -q <(norm Y.$n.err) <(norm Y0.$n.err) > /dev/null; then
    r=same
  else
    r="DIFFERS rc $(cat Y0.$n.rc)/$(cat Y.$n.rc) out-lines $(wc -l < Y0.$n.out)/$(wc -l < Y.$n.out) err-lines $(wc -l < Y0.$n.err)/$(wc -l < Y.$n.err)"
  fi
  printf '%-4s %-20s %s\n' "$T" "$n" "$r"
done
echo "elflint on Y (filler complaints):"; grep -c 'st_value out of bounds' Y.elflint.out | sed 's/^/  st_value out of bounds lines: /'
