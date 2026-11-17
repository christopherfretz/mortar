#!/bin/bash
# review/anchors.sh WORK: record §G Q10: MT-16 section anchors of a kiln++ unit after
# -r (GNU ld, lld) and objcopy, and what each way of deciding "do they still hold" says.
set -u; ulimit -c 0
H=$(cd "$(dirname "$0")" && pwd); W=$(realpath -m "$1"); mkdir -p "$W"; cd "$W" || exit 1
K=/Volumes/AgentWork/mortar-helper/.build/target-kiln-next/release/kiln++
$K -O1 -c -fmason-metadata=detail -o k1.o "$H/../meta/m1.cc"; $K -O1 -c -fmason-metadata=detail -o k2.o "$H/../meta/m2.cc"
printf 'int g(int x){return x*3;}\nconst char *gs = "gcc";\n' > g.c; gcc -O1 -c g.c -o g.o
printf 'abcd' > blob
t() { local n=$1; shift; "$@" > /dev/null 2>&1 || { echo "$n: the tool failed"; return; }; echo "$n"; python3 -I "$H/anchors.py" k1.o out.$n.o; }
t T0-unchanged cp k1.o out.T0-unchanged.o
t T1-bfd-r-alone ld.bfd -r -o out.T1-bfd-r-alone.o k1.o
t T2-bfd-r-gcc-first ld.bfd -r -o out.T2-bfd-r-gcc-first.o g.o k1.o
t T2-lld-r-gcc-first ld.lld -r -o out.T2-lld-r-gcc-first.o g.o k1.o
t T3-bfd-r-kiln-first ld.bfd -r -o out.T3-bfd-r-kiln-first.o k1.o g.o
t T4-bfd-r-two-units ld.bfd -r -o out.T4-bfd-r-two-units.o k2.o k1.o
t T5-objcopy-remove-comment objcopy --remove-section .comment k1.o out.T5-objcopy-remove-comment.o
t T6-objcopy-rename-text objcopy --rename-section .text=.text.moved k1.o out.T6-objcopy-rename-text.o
echo "anchors done"
