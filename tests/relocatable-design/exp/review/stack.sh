#!/bin/bash
# review/stack.sh WORK: .note.GNU-stack at -r, per target x input set x -z option x
# reference. Inputs are assembled so the note is exactly N (no SHF_EXECINSTR),
# X (SHF_EXECINSTR) or A (absent). Cell: "-" no note, "N", "X", or "err".
set -u; ulimit -c 0
W=$(realpath -m "$1"); mkdir -p "$W"; cd "$W" || exit 1
for T in aarch64 x86_64 i686; do
  case $T in aarch64) AS=as BFD=ld.bfd LLD="ld.lld -m aarch64linux" MOLD="mold -m aarch64linux"; R=ret;;
             x86_64) AS=x86_64-linux-gnu-as BFD=x86_64-linux-gnu-ld LLD="ld.lld -m elf_x86_64" MOLD="mold -m elf_x86_64"; R=ret;;
             i686) AS=i686-linux-gnu-as BFD=i686-linux-gnu-ld LLD="ld.lld -m elf_i386" MOLD="mold -m elf_i386"; R=ret;; esac
  for k in N X A; do for i in 1 2; do
    { printf '.text\n.globl f%s%s\nf%s%s: %s\n' $k $i $k $i $R
      case $k in N) printf '.section .note.GNU-stack,"",%%progbits\n';; X) printf '.section .note.GNU-stack,"x",%%progbits\n';; esac; } | $AS -o $T.$k$i.o; done; done
  printf '%-7s %-4s %-15s %-5s %-5s %-5s\n' target set option bfd lld mold
  for set in NN XX NX NA XA AA; do for z in "" "-z execstack" "-z noexecstack"; do
    row=""; for L in bfd lld mold; do case $L in bfd) LD=$BFD;; lld) LD=$LLD;; mold) LD=$MOLD;; esac
      o=$T.$set.$L.$(echo "x$z" | tr -dc 'a-z').o; ins="$T.${set:0:1}1.o $T.${set:1:1}2.o"
      if $LD -r $z -o $o $ins > /dev/null 2>&1; then
        c=$(readelf -SW $o | grep -E '\.note\.GNU-stack' | sed -E 's/^.*PROGBITS +[0-9a-f]+ +[0-9a-f]+ +[0-9a-f]+ +[0-9a-f]+ +//' | awk '{print ($1 ~ /X/) ? "X" : "N"}'); row="$row ${c:--}"
      else row="$row err"; fi; done
    printf '%-7s %-4s %-15s %s\n' $T $set "${z:-(none)}" "$row"; done; done; done
echo "stack done"
