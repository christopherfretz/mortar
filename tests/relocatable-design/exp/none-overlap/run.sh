#!/bin/bash
# none-overlap/run.sh WORK: GNU ld's -r output of two C++ objects sharing an
# inline function (-O0: a COMDAT .text with its own FDE) drops the losing
# copy's FDE and turns its .eh_frame relocation into R_*_NONE, whose r_offset
# can equal a live record's. mortar's final link refuses it ("overlapping
# relocations"); GNU ld's final link runs.
set -u; ulimit -c 0
WORK=$(realpath -m "$1"); M=${2:-/Volumes/AgentWork/mortar-helper/.build/target-wp-154/mortar-3ad9867a}
mkdir -p "$WORK/mbin"; cd "$WORK" || exit 1
for n in ld x86_64-linux-gnu-ld i686-linux-gnu-ld; do ln -sf "$M" mbin/$n; done
printf 'inline int f(int x){static int k; return k += x;}\nint g1(int x){return f(x);}\n' > g1.cc
printf 'int g3(int x){return x;}\ninline int f(int x){static int k; return k += x;}\nint g2(int x){return f(x)+g3(1);}\n' > g2.cc
printf '#include <cstdio>\nint g1(int); int g2(int);\nint main(){ std::printf("%%d\\n", g1(1) + g2(2)); }\n' > m.cc
for a in aarch64 x86_64 i686; do
  case $a in aarch64) CXX=g++; LD=ld.bfd; Q="";; x86_64) CXX=x86_64-linux-gnu-g++; LD=x86_64-linux-gnu-ld.bfd; Q="qemu-x86_64 -L /usr/x86_64-linux-gnu";; i686) CXX=i686-linux-gnu-g++; LD=i686-linux-gnu-ld.bfd; Q="qemu-i386 -L /usr/i686-linux-gnu";; esac
  for s in g1 g2 m; do $CXX -O0 -c $s.cc -o $a.$s.o; done
  $LD -r -o $a.r.o $a.g1.o $a.g2.o
  echo "$a: NONE records in .eh_frame: $(readelf -rW $a.r.o | awk '/eh_frame/{f=1;next} /Relocation section/{f=0} f && /NONE/' | wc -l); shared offsets: $(readelf -rW $a.r.o | awk '/eh_frame/{f=1;next} /Relocation section/{f=0} f && /R_/{print $1}' | sort | uniq -d | tr '\n' ' ')"
  $CXX -static $a.m.o $a.r.o -o $a.p.bfd && echo "  final GNU ld: $($Q ./$a.p.bfd)"
  $CXX -static -B$WORK/mbin/ $a.m.o $a.r.o -o $a.p.mortar 2>&1 | head -1 | sed 's/^/  final mortar: /'; [ -f $a.p.mortar ] && echo "  final mortar: $($Q ./$a.p.mortar)"
  $CXX -static -B$WORK/mbin/ $a.m.o $a.g1.o $a.g2.o -o $a.d.mortar && echo "  direct mortar: $($Q ./$a.d.mortar)"
done
