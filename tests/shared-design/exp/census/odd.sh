#!/bin/bash
# odd.sh WORK: the two -shared driver lines of part 1 that GNU ld links
# (with warnings only) and whose library then misbehaves when loaded, kept
# with their outputs and compared against lld 22 and mold.
#  A: clang (19, 22) --target=i686-linux-gnu, C++ object compiled -fno-pic, -shared
#  B: i686-linux-gnu-g++ -shared -static
set -u
ulimit -c 0
WORK=$(realpath -m "$1"); HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$WORK/tmp"; cd "$WORK" || exit 1; export TMPDIR=$WORK/tmp
Q="qemu-i386 -L /usr/i686-linux-gnu"
i686-linux-gnu-gcc "$HERE/src/host.c" -o host
run() { local o rc; o=$($Q ./host "$WORK/$1" 2>&1); rc=$?; echo "  run: $(echo "$o" | tr '\n' '|' | cut -c1-300) rc=$rc"; }
facts() { echo "  facts: $(readelf -dW "$1" | grep -E 'TEXTREL|FLAGS' | awk '{print $2 $3}' | tr '\n' ' ') phdrs: $(readelf -lW "$1" | awk '/^  [A-Z]/{print $1}' | sort -u | tr '\n' ' ') dynrel-against-.eh_frame: $(readelf -rW "$1" | awk 'NR>2' | wc -l) relocs total; _Unwind dyn: $(readelf --dyn-syms -W "$1" | awk '$NF ~ /_Unwind_RaiseException|__register_frame_info/ {print $NF "(" $(NF-1) ")"}' | tr '\n' ' ')"; }
echo "=== A: C++ -fno-pic object into -shared, i686"
for cc in clang++-19 clang++-22 i686-linux-gnu-g++; do
  t=""; case $cc in clang*) t=--target=i686-linux-gnu;; esac
  $cc $t -fno-pic -c "$HERE/src/lib.cpp" -o "A.$cc.o"
  readelf -rW "A.$cc.o" | awk '/Relocation section/{s=$3} /R_386/{c[s" "$3]++} END {for (k in c) print "  obj reloc", k, c[k]}' | sort
  for k in bfd lld mold; do
    case $k in bfd) lf="";; lld) case $cc in clang*) lf="-fuse-ld=lld --ld-path=/usr/bin/ld.lld-22";; *) mkdir -p lld22bin; ln -sf /usr/bin/ld.lld-22 lld22bin/ld.lld; lf="-B$WORK/lld22bin/ -fuse-ld=lld";; esac;; mold) lf="-fuse-ld=mold";; esac
    msg=$($cc $t -shared $lf "A.$cc.o" -o "A.$cc.$k.so" 2>&1); rc=$?
    echo "-- $cc + $k: link rc=$rc :: $(echo "$msg" | grep -v '^$' | head -4 | tr '\n' '|' | cut -c1-400)"
    [ $rc = 0 ] && { facts "A.$cc.$k.so"; run "A.$cc.$k.so"; }
  done
done
echo "=== A': the same clang object, -shared with -z notext (lld) / mold -z notext"
msg=$(clang++-22 --target=i686-linux-gnu -shared -fuse-ld=lld --ld-path=/usr/bin/ld.lld-22 -Wl,-z,notext A.clang++-22.o -o A.lld-notext.so 2>&1); echo "-- lld -z notext rc=$? $(echo "$msg" | head -2 | tr '\n' '|')"; [ -e A.lld-notext.so ] && { facts A.lld-notext.so; run A.lld-notext.so; }
echo "=== B: i686-linux-gnu-g++ -shared -static (C++), and gcc C"
i686-linux-gnu-g++ -fPIC -c "$HERE/src/lib.cpp" -o B.o
i686-linux-gnu-gcc -fPIC -c "$HERE/src/lib.c" -o Bc.o
for k in bfd lld mold; do
  case $k in bfd) lf="";; lld) mkdir -p lld22bin; ln -sf /usr/bin/ld.lld-22 lld22bin/ld.lld; lf="-B$WORK/lld22bin/ -fuse-ld=lld";; mold) lf="-fuse-ld=mold";; esac
  msg=$(i686-linux-gnu-g++ -shared -static $lf B.o -o "B.$k.so" 2>&1); rc=$?
  echo "-- g++ -shared -static + $k: rc=$rc :: $(echo "$msg" | grep -v '^$' | head -3 | tr '\n' '|' | cut -c1-400)"
  [ $rc = 0 ] && { facts "B.$k.so"; run "B.$k.so"; }
  msg=$(i686-linux-gnu-gcc -shared -static $lf Bc.o -o "Bc.$k.so" 2>&1); rc=$?
  echo "-- gcc -shared -static (C) + $k: rc=$rc :: $(echo "$msg" | grep -v '^$' | head -3 | tr '\n' '|' | cut -c1-300)"
  [ $rc = 0 ] && { facts "Bc.$k.so"; run "Bc.$k.so"; }
done
echo "=== B': GNU ld line of B with --eh-frame-hdr added (does the unwinder then find the library's FDEs?)"
i686-linux-gnu-g++ -shared -static -Wl,--eh-frame-hdr B.o -o B.ehfh.so 2>/dev/null; echo "rc=$?"; [ -e B.ehfh.so ] && { facts B.ehfh.so; run B.ehfh.so; }
