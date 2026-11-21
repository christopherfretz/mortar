#!/bin/sh
# Regenerates WP-210's fixtures of tests/commonsec.rs: an input section
# named exactly `COMMON` (WP-203's verifier gaps), -nostdlib, static,
# AArch64 and x86-64, by lld, GNU ld and mold (each output with its map
# and the words its linker was given, `.line`):
#   nobits    start.o vn.o              records into a NOBITS COMMON (R5)
#   progbits  start.o vp.o              .bss PROGBITS (GNU ld: A129)
#   dotx      start.o vx.o              orphan COMMON.x after .bss (A130)
#   split     start.o vp.o vn.o         PROGBITS and NOBITS COMMON (mold: A131)
#   gc        --gc-sections startg.o vn.o vg.o  GNU ld's discarded COMMON row
#             (startg: no __start_COMMON reference, which GNU ld keeps X for)
# The outputs are checked in.
#   sh gen.sh
# Run from this directory.
set -e
for a in a64 x64; do
  case $a in
    a64) cc=aarch64-linux-gnu-gcc; bfd=aarch64-linux-gnu-ld.bfd; em=aarch64linux;;
    x64) cc=x86_64-linux-gnu-gcc; bfd=x86_64-linux-gnu-ld.bfd; em=elf_x86_64;;
  esac
  $cc -c start-$a.s -o start-$a.o
  $cc -c startg-$a.s -o startg-$a.o
  for v in vn vp vx vg; do $cc -c $v.s -o $v-$a.o; done
  for c in nobits progbits dotx split gc; do
    case $c in
      nobits) w="start-$a.o vn-$a.o";;
      progbits) w="start-$a.o vp-$a.o";;
      dotx) w="start-$a.o vx-$a.o";;
      split) w="start-$a.o vp-$a.o vn-$a.o";;
      gc) w="--gc-sections startg-$a.o vn-$a.o vg-$a.o";;
    esac
    for l in lld bfd mold; do
      case $l in
        lld) prog=ld.lld-19; m="";;
        bfd) prog=$bfd; m="";;
        mold) prog=ld.mold; m="-m $em";;
      esac
      # shellcheck disable=SC2086
      set -- $m -static -z max-page-size=4096 -o $c-$a.$l $w -Map=$c-$a.$l.map
      printf '%s\n' "$@" > $c-$a.$l.line
      $prog "$@"
    done
  done
done
