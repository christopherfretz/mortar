#!/bin/bash
# review/mapsym.sh WORK: AArch64 mapping symbols ($d) in .eh_frame and .note.gnu.property
# after each reference's -r, where one input's FDE (the dropped COMDAT copy's) is removed.
set -u; ulimit -c 0
W=$(realpath -m "$1"); mkdir -p "$W"; cd "$W" || exit 1
printf 'inline int sh(int x){return x+1;}\nint a1(int y){return sh(y);}\n' > a.cc
printf 'inline int sh(int x){return x+1;}\nint b1(int y){return sh(y)*2;}\n' > b.cc
for f in a b; do g++ -O0 -mbranch-protection=standard -c $f.cc -o $f.o; done
echo "a.o:"; readelf -sW a.o | grep -E '\$d'; readelf -SW a.o | grep -E '\] \.(eh_frame|note.gnu.property)' | sed -E 's/^ *\[ *([0-9]+)\] +([^ ]+).*/  section \1 = \2/'
echo "b.o .eh_frame records:"; readelf --debug-dump=frames b.o | grep -E '^[0-9a-f]+ '
for L in bfd lld mold; do case $L in bfd) c=ld.bfd;; lld) c=ld.lld;; mold) c=mold;; esac; $c -r -o r.$L.o a.o b.o
  echo "== $L: $(readelf -SW r.$L.o | grep -E '\] \.(eh_frame|note.gnu.property)' | sed -E 's/^ *\[ *([0-9]+)\] +([^ ]+).*/\1=\2/' | tr '\n' ' ')"; readelf -sW r.$L.o | grep -E '\$d'
  [ $L = bfd ] && { echo "  bfd .eh_frame records:"; readelf --debug-dump=frames r.$L.o | grep -E '^[0-9a-f]+ '; }; done
echo "mapsym done"
