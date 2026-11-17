#!/bin/sh
# Regenerates WP-166's fixtures of tests/ldef.rs: dyn.md D-110 item 2 and
# D-21 item 2 (WP-150 §3a), -nostdlib, AArch64 and x86-64, by mortar,
# lld, GNU ld and mold (each output with its map and, a reference's, the
# words its linker was given, `.line`):
#   sh, shsym, shfn, shloc  -shared lib.o: plain, -Bsymbolic,
#                           -Bsymbolic-functions, local: * (local.map)
#   shx                     -shared libx.o: __etext and the AArch64 names
#   exref-pie, exref-nopie  start.o against libend.so (references the seven)
#   exdef-pie, exdef-nopie  start(DEF).o against libdef.so (defines them)
#   exdefr-pie, exdefr-nopie  startr.o (references them) against libdef.so
#   pie-e                   -pie -E startr.o: the GOT slot of _end
# The outputs are checked in.
#   sh gen.sh <mortar>
# Run from this directory. First made with the mortar of WP-166's base
# (318dc90d).
set -e
mortar=$1
for a in a64 x64; do
  case $a in
    a64) cc=aarch64-linux-gnu-gcc; bfd=aarch64-linux-gnu-ld.bfd; ld=/lib/ld-linux-aarch64.so.1; em=aarch64linux;;
    x64) cc=x86_64-linux-gnu-gcc; bfd=x86_64-linux-gnu-ld.bfd; ld=/lib64/ld-linux-x86-64.so.2; em=elf_x86_64;;
  esac
  f="-O1 -fno-asynchronous-unwind-tables -fno-stack-protector"
  $cc $f -fPIC -c lib.c -o lib-$a.o
  $cc $f -fPIC -c libx.c -o libx-$a.o
  $cc $f -fPIC -c libdef.c -o libdef-$a.o
  $cc $f -fPIE -c start.c -o start-$a.o
  $cc $f -fPIE -DDEF -c start.c -o start-def-$a.o
  $cc $f -fPIE -c startr.c -o startr-$a.o
  $bfd -shared -z max-page-size=4096 -soname libend.so -o libend-$a.so lib-$a.o
  $bfd -shared -z max-page-size=4096 -soname libdef.so -o libdef-$a.so libdef-$a.o
  for c in sh shsym shfn shloc shx exref-pie exref-nopie exdef-pie exdef-nopie exdefr-pie exdefr-nopie pie-e; do
    case $c in
      sh) w="-shared lib-$a.o";;
      shsym) w="-shared -Bsymbolic lib-$a.o";;
      shfn) w="-shared -Bsymbolic-functions lib-$a.o";;
      shloc) w="-shared --version-script=local.map lib-$a.o";;
      shx) w="-shared libx-$a.o";;
      exref-pie) w="-pie -dynamic-linker $ld start-$a.o libend-$a.so";;
      exref-nopie) w="-no-pie -dynamic-linker $ld start-$a.o libend-$a.so";;
      exdef-pie) w="-pie -dynamic-linker $ld start-def-$a.o libdef-$a.so";;
      exdef-nopie) w="-no-pie -dynamic-linker $ld start-def-$a.o libdef-$a.so";;
      exdefr-pie) w="-pie -dynamic-linker $ld startr-$a.o libdef-$a.so";;
      exdefr-nopie) w="-no-pie -dynamic-linker $ld startr-$a.o libdef-$a.so";;
      pie-e) w="-pie -E -dynamic-linker $ld startr-$a.o";;
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
