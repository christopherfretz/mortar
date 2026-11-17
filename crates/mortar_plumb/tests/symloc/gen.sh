#!/bin/sh
# Regenerates WP-169's fixtures of tests/symloc.rs: synth S-75, S-77,
# S-121, dyn.md D-114 (WP-150), -nostdlib, AArch64 and x86-64, by mortar,
# lld, GNU ld and mold (each output with its map and, a reference's, the
# words its linker was given, `.line`):
#   vl          -shared --version-script=vl.map vl.o: version-local keys,
#               a local common, __start_mysec, _end, __bss_start, no .data
#   gc          -shared vl.o: the same common global
#   sv          -shared --version-script=sv.map sv.o: foo@V1, foo@@V2
#   exl-pie, exl-nopie  -E ex.o libx.a libdep.so --exclude-libs=ALL
# The outputs are checked in.
#   sh gen.sh <mortar>
# Run from this directory. First made with the mortar of WP-169's base
# (d03114c6).
set -e
mortar=$1
for a in a64 x64; do
  case $a in
    a64) cc=aarch64-linux-gnu-gcc; ar=aarch64-linux-gnu-ar; bfd=aarch64-linux-gnu-ld.bfd; ld=/lib/ld-linux-aarch64.so.1; em=aarch64linux;;
    x64) cc=x86_64-linux-gnu-gcc; ar=x86_64-linux-gnu-ar; bfd=x86_64-linux-gnu-ld.bfd; ld=/lib64/ld-linux-x86-64.so.2; em=elf_x86_64;;
  esac
  f="-O1 -fno-asynchronous-unwind-tables -fno-stack-protector"
  $cc $f -fPIC -fcommon -c vl.c -o vl-$a.o
  $cc $f -fPIC -c sv.c -o sv-$a.o
  $cc $f -fPIE -c ex.c -o ex-$a.o
  $cc $f -fPIE -c exlib.c -o exlib-$a.o
  $ar rcs libx-$a.a exlib-$a.o
  $cc $f -fPIC -c dep.c -o dep-$a.o
  $bfd -shared -z max-page-size=4096 -soname libdep.so -o libdep-$a.so dep-$a.o
  for c in vl gc sv exl-pie exl-nopie; do
    case $c in
      vl) w="-shared --version-script=vl.map vl-$a.o";;
      gc) w="-shared vl-$a.o";;
      sv) w="-shared --version-script=sv.map sv-$a.o";;
      exl-pie) w="-pie -E -dynamic-linker $ld ex-$a.o libx-$a.a libdep-$a.so --exclude-libs=ALL";;
      exl-nopie) w="-no-pie -E -dynamic-linker $ld ex-$a.o libx-$a.a libdep-$a.so --exclude-libs=ALL";;
    esac
    # shellcheck disable=SC2086
    "$mortar" -o $c-$a $w --mortar-map=$c-$a.map
    for l in lld bfd mold; do
      case $l in
        lld) prog=ld.lld-19; m="";;
        bfd) prog=$bfd; m="";;
        mold) prog=ld.mold; m="-m $em";;
      esac
      # shellcheck disable=SC2086
      set -- $m -z max-page-size=4096 -o $c-$a.$l $w -Map=$c-$a.$l.map
      printf '%s\n' "$@" > $c-$a.$l.line
      $prog "$@"
    done
  done
done
