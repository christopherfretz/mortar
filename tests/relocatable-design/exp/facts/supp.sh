#!/bin/bash
# facts/supp.sh WORK: the cells run.sh got wrong or missed (run after run.sh in
# the same WORK; it reuses run.sh's objects): addrsig/comment with distinct
# symbols, CREL inputs, x86-64 GOTPCRELX, nested -r, EI_OSABI, build-id notes
# from -r inputs, -r then the mortar final link of a CREL-free nested output.
set -u; ulimit -c 0
WORK=$(realpath -m "$1"); HERE=$(cd "$(dirname "$0")" && pwd); T=$HERE/../tools
M=/Volumes/AgentWork/mortar-helper/.build/target-wp-154/mortar-3ad9867a
cd "$WORK" || exit 1
eval "$(sed -n '/^R() {/,/^}/p; /^secs() /p; /^syms() /p; /^secs_p() /p; /^syms_p() /p; /^each() {/,/^}/p' "$HERE/run.sh")"
dac() { secs $1 "addrsig|comment"; echo -n " comment: $(readelf -p .comment $1 2>/dev/null | grep "\]" | tr -s " " | cut -c1-60 | tr "\n" "|")"; }
each 4b-addrsig-comment aarch64 dac cb1.o b0.o
clang-22 -O1 -c -Wa,--crel,--allow-experimental-crel r1.c -o crel.o
echo "9-crel-input | - | $(readelf -SW crel.o | grep -c crel) .crel sections"
crd() { echo -n "$(readelf -SW $1 | awk '/\.(crel|rela?)\./{print $2"("$3")"}' | tr '\n' ' ')"; }
each 9-crel aarch64 crd crel.o
x86_64-linux-gnu-gcc -O1 -fPIC -c r1.c -o xp1.o
rtypes() { readelf -rW $1 | awk '/R_/{print $3}' | sort | uniq -c | awk '{printf "%s:%s ", $2, $1}'; }
echo "9-gotpcrelx-input | - | $(rtypes xp1.o)"
each 9-gotpcrelx x86_64 rtypes xp1.o
# nested -r: a -r output as an input of -r, with a group whose signature the other input repeats
for k in bfd lld mold; do
  R aarch64 $k n1.$k.o g1.o y1.o 2>/dev/null; R aarch64 $k n2.$k.o n1.$k.o g2.o y2.o 2>/dev/null; R aarch64 $k n0.$k.o g1.o y1.o g2.o y2.o 2>/dev/null
  echo "9-nested | $k | groups nested $(readelf -gW n2.$k.o | grep -c COMDAT) flat $(readelf -gW n0.$k.o | grep -c COMDAT); secs nested $(readelf -SW n2.$k.o | grep -c '^  \[') flat $(readelf -SW n0.$k.o | grep -c '^  \['); rcheck: $(python3 -I $T/rcheck.py n2.$k.o n1.$k.o g2.o y2.o | grep -E 'MISSING|DIFFERS|unexplained|equal' | tr -s ' ' | tr '\n' ';')"
done
# EI_OSABI: an input with STB_GNU_UNIQUE (GNU ABI) and one without
for o in yu.o 5-ifunc-unique.bfd.o 5-ifunc-unique.lld.o; do [ -f $o ] && echo "9-osabi | $o | $(readelf -hW $o | awk -F: '/OS\/ABI/{print $2}' | tr -s ' ')"; done
# build-id: clang -r output with a note, then a final link with --build-id
printf 'int main(void){return 0;}\n' > bm.c; gcc -c bm.c -o bm.o
ld.bfd -r --build-id -o bid.o bm.o
for L in bfd mortar; do
  if [ $L = bfd ]; then ld.bfd --build-id -e main -o bid.$L bid.o 2>/dev/null; else $M --build-id -e main -o bid.$L bid.o > bid.$L.log 2>&1; fi
  echo "9-buildid-final | $L | rc=$? notes: $(readelf -nW bid.$L 2>/dev/null | grep -c 'Build ID') $(head -1 bid.$L.log 2>/dev/null)"
done
echo "supp done"
