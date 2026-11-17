#!/bin/bash
# rl0/none.sh WORK: after the supervisor's ruling on WP-180 Q-R1 (input
# R_*_NONE records are carried at -r). Per target: (1) `.reloc ., R_*_NONE,
# keepme` in _start's object, keepme in its own section of a second object;
# each reference's -r of the two, then a --gc-sections final link of the -r
# output by GNU ld and lld: is keepme kept (nm), and how many NONE records
# the -r output holds; the direct link and a link without the record for
# comparison. (2) clang's .llvm.call-graph-profile on x86-64 and i386: the
# type of its relocation section. Log: tests/relocatable-design/work/rl0-none.log.
set -u; ulimit -c 0
WORK=$(realpath -m "$1"); mkdir -p "$WORK/none"; cd "$WORK/none" || exit 1
for a in aarch64 x86_64 i386; do
  case $a in
    aarch64) T=aarch64-linux-gnu; N=R_AARCH64_NONE; BFD=ld.bfd; EM=aarch64linux; RET=ret;;
    x86_64) T=x86_64-linux-gnu; N=R_X86_64_NONE; BFD=x86_64-linux-gnu-ld.bfd; EM=elf_x86_64; RET=ret;;
    i386) T=i686-linux-gnu; N=R_386_NONE; BFD=i686-linux-gnu-ld.bfd; EM=elf_i386; RET=ret;;
  esac
  printf '  .text\n  .globl _start\n_start:\n  .reloc ., %s, keepme\n  %s\n' "$N" "$RET" > s.$a.s
  printf '  .text\n  .globl _start\n_start:\n  %s\n' "$RET" > p.$a.s
  printf '  .section .text.keepme,"ax"\n  .globl keepme\nkeepme:\n  %s\n' "$RET" > k.$a.s
  for f in s p k; do clang --target=$T -c $f.$a.s -o $f.$a.o || exit 1; done
  echo "== $a input NONE records: $(readelf -rW s.$a.o | grep -c NONE)"
  for fin in bfd lld; do
    case $fin in bfd) F="$BFD";; lld) F="ld.lld-22 -m $EM";; esac
    $F --gc-sections -e _start -o d.$a.$fin s.$a.o k.$a.o; echo "direct $a final=$fin keepme: $(nm d.$a.$fin | grep -c keepme)"
    $F --gc-sections -e _start -o n.$a.$fin p.$a.o k.$a.o; echo "no-record $a final=$fin keepme: $(nm n.$a.$fin | grep -c keepme)"
  done
  for r in bfd lld mold; do
    case $r in bfd) $BFD -r -o r.$a.$r.o s.$a.o k.$a.o;; lld) ld.lld-22 -m $EM -r -o r.$a.$r.o s.$a.o k.$a.o;; mold) mold -m $EM -r -o r.$a.$r.o s.$a.o k.$a.o;; esac
    echo "-r by $r rc=$? NONE records: $(readelf -rW r.$a.$r.o | grep -c NONE)"
    for fin in bfd lld; do
      case $fin in bfd) F="$BFD";; lld) F="ld.lld-22 -m $EM";; esac
      $F --gc-sections -e _start -o v.$a.$r.$fin r.$a.$r.o 2>&1 | head -1
      echo "  via $r -r, final=$fin keepme: $(nm v.$a.$r.$fin 2>/dev/null | grep -c keepme)"
    done
  done
done
echo "== cg_profile relocation section type"
printf '  .text\n  .globl cga, cgb\ncga: ret\ncgb: ret\n  .cg_profile cga, cgb, 10\n' > cg.s
for T in aarch64-linux-gnu x86_64-linux-gnu i686-linux-gnu; do
  clang --target=$T -c cg.s -o cg.$T.o && echo "$T: $(readelf -SW cg.$T.o | grep -o 'rel[a]*\.llvm.call-graph-profile *RELA*' | tr -s ' ')"
done
echo "none done"
