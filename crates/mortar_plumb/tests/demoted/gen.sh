#!/bin/sh
# Regenerates WP-178's fixtures of tests/demoted.rs (spec/plumb.md §5.7a,
# A103): a reference through a weak mention defined only in a dropped
# COMDAT copy whose key nothing live defines, -nostdlib:
#   st-a64, st-x64   wm.o wa.o wb.o, static: wb.o's Y calls its demoted hx
#                    (GNU ld's and lld's outputs, maps and words, `.line`)
#   pie-a64          wad.o wd.o, -pie: wd.o's Y reads hx through the GOT
#                    (GNU ld's; its import and GLOB_DAT)
#   mo-a64           mortar's output of wm.o wa.o wbu.o (wbu.o: wb.S with
#                    hx only an undefined weak reference, the same sections),
#                    judged in tests/demoted.rs with wb.o in wbu.o's place:
#                    the image mortar would write if it took the demoted
#                    mention as GNU ld does (mortar refuses the real link)
#   sh gen.sh <mortar>
# Run from this directory. First made with the mortar of WP-178's base
# (int/batch-11, ef847a2f).
set -e
mortar=$1
for a in a64 x64; do
  case $a in
    a64) cc=aarch64-linux-gnu-gcc; bfd=aarch64-linux-gnu-ld.bfd;;
    x64) cc=x86_64-linux-gnu-gcc; bfd=x86_64-linux-gnu-ld.bfd;;
  esac
  f="-O1 -fno-asynchronous-unwind-tables -fno-stack-protector -fno-pie"
  $cc $f -c wm.c -o wm-$a.o
  $cc -c wa.S -o wa-$a.o
  $cc -c wb.S -o wb-$a.o
  for l in lld bfd; do
    case $l in lld) prog=ld.lld-19;; bfd) prog=$bfd;; esac
    set -- -static -z max-page-size=4096 -o st-$a.$l wm-$a.o wa-$a.o wb-$a.o -Map=st-$a.$l.map
    printf '%s\n' "$@" > st-$a.$l.line
    $prog "$@"
  done
done
cc=aarch64-linux-gnu-gcc
f="-O1 -fno-asynchronous-unwind-tables -fno-stack-protector -fPIE"
$cc $f -c wad.c -o wad-a64.o
$cc $f -c wd.c -o wd-a64.o
set -- -pie -z max-page-size=4096 -o pie-a64.bfd -dynamic-linker /lib/ld-linux-aarch64.so.1 wad-a64.o wd-a64.o -Map=pie-a64.bfd.map
printf '%s\n' "$@" > pie-a64.bfd.line
aarch64-linux-gnu-ld.bfd "$@"
$cc -DUNDEF -c wb.S -o wbu-a64.o
"$mortar" -static -o mo-a64 wm-a64.o wa-a64.o wbu-a64.o --mortar-map=mo-a64.map
