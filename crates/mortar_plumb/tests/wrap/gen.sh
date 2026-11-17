#!/bin/sh
# Regenerates WP-170's fixtures of tests/wrap.rs: --wrap=SYM
# (spec/resolve.md §7.1, spec/plumb.md §4.4a) by mortar, lld, GNU ld and
# mold (each output with its map and, a reference's, the words its linker
# was given, `.line`), -nostdlib:
#   st      -static -e _start m.o f.o w.o --wrap=f: f, g (f's own object,
#           E13), __real_f, __wrap_f
#   mem     -static mf.o w7.o libfo.a --wrap=f: a member that defines only
#           f is not extracted
#   real    -static mr.o libfo.a --wrap=f: __real_f extracts it
#   gc      -static --gc-sections gcm.o gcf.o --wrap=f: f's section is
#           collected (only a wrapped f names it)
#   gck     the same without --gc-sections (tests/wrap.rs judges it by
#           gc's line: f's section kept)
#   dm      -pie dm.o libmal.so --wrap=malloc --wrap=_start (AArch64 and
#           x86-64 only; the entry stays _start)
# on AArch64 (a64), x86-64 (x64) and i386 (x86). The outputs are checked in.
#   sh gen.sh <mortar>
# Run from this directory. First made with the mortar of WP-170's base
# (f20a6d6c).
set -e
mortar=$1
for a in a64 x64 x86; do
  case $a in
    a64) cc=aarch64-linux-gnu-gcc; ar=aarch64-linux-gnu-ar; bfd=aarch64-linux-gnu-ld.bfd; ld=/lib/ld-linux-aarch64.so.1; em=aarch64linux;;
    x64) cc=x86_64-linux-gnu-gcc; ar=x86_64-linux-gnu-ar; bfd=x86_64-linux-gnu-ld.bfd; ld=/lib64/ld-linux-x86-64.so.2; em=elf_x86_64;;
    x86) cc=i686-linux-gnu-gcc; ar=i686-linux-gnu-ar; bfd=i686-linux-gnu-ld.bfd; ld=/lib/ld-linux.so.2; em=elf_i386;;
  esac
  f="-O1 -fno-inline -fno-unwind-tables -fno-asynchronous-unwind-tables -fno-stack-protector"
  for s in m f w mf w7 mr fo gcm; do
    $cc $f -fno-pic -c $s.c -o $s-$a.o
  done
  $cc $f -fno-pic -ffunction-sections -c fo.c -o gcf-$a.o
  $ar rcs libfo-$a.a fo-$a.o
  cells="st mem real gc gck"
  if [ $a != x86 ]; then
    $cc $f -fPIE -c dm.c -o dm-$a.o
    $cc $f -fPIC -c mal.c -o mal-$a.o
    $bfd -shared -z max-page-size=4096 -soname libmal.so -o libmal-$a.so mal-$a.o
    cells="$cells dm"
  fi
  for c in $cells; do
    case $c in
      st) w="-static -e _start m-$a.o f-$a.o w-$a.o --wrap=f";;
      mem) w="-static -e _start mf-$a.o w7-$a.o libfo-$a.a --wrap=f";;
      real) w="-static -e _start mr-$a.o libfo-$a.a --wrap=f";;
      gc) w="-static -e _start --gc-sections gcm-$a.o gcf-$a.o --wrap=f";;
      gck) w="-static -e _start gcm-$a.o gcf-$a.o --wrap=f";;
      dm) w="-pie -dynamic-linker $ld dm-$a.o libmal-$a.so --wrap=malloc --wrap=_start";;
    esac
    # shellcheck disable=SC2086
    "$mortar" -m $em -z max-page-size=4096 -o $c-$a $w --mortar-map=$c-$a.map
    for l in lld bfd mold; do
      case $l in
        lld) prog=ld.lld-19;;
        bfd) prog=$bfd;;
        mold) prog=ld.mold;;
      esac
      # shellcheck disable=SC2086
      set -- -m $em -z max-page-size=4096 -o $c-$a.$l $w -Map=$c-$a.$l.map
      printf '%s\n' "$@" > $c-$a.$l.line
      $prog "$@"
    done
  done
done
