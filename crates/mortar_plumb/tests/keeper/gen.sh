#!/bin/sh
# Regenerates WP-210's fixtures of tests/keeper.rs: the COMDAT keeper
# among one archive's members (spec/plumb.md §5.7a). lib.a holds m1.o
# then m2.o, both with group `g`; main.o calls f2 (m2.o), which calls f1
# (m1.o), g and h; m1.o's copy of the group also defines h, and libh.so
# (given --as-needed) defines it too. So m2.o is extracted first and m1.o
# second, and the linkers differ in which copy they keep. libs.a is
# lib.a with m2s.o, m2.o without the call to h (a static link has no
# libh.so):
#   st-<arch>.<linker>    -static main.o libs.a       (bfd lld mold mortar)
#   pie-<arch>.<linker>   -pie main.o lib.a --as-needed libh.so
#                         (bfd mold mortar; lld refuses it: "relocation
#                         refers to a symbol in a discarded section: h")
# Each output has its map and the words its linker was given (`.line`).
# The outputs are checked in.
#   sh gen.sh <mortar>
# Run from this directory (`ar r` replaces the archives' members in
# place). First made with the mortar of WP-210's base (1bfce906).
set -e
mortar=$1
for a in a64 x64; do
  case $a in
    a64) cc=aarch64-linux-gnu-gcc; bfd=aarch64-linux-gnu-ld.bfd; em=aarch64linux
         dl=/lib/ld-linux-aarch64.so.1;;
    x64) cc=x86_64-linux-gnu-gcc; bfd=x86_64-linux-gnu-ld.bfd; em=elf_x86_64
         dl=/lib64/ld-linux-x86-64.so.2;;
  esac
  for f in main m1 m2; do $cc -c $f-$a.s -o $f-$a.o; done
  ar rcs lib-$a.a m1-$a.o m2-$a.o
  $cc -c m2s-$a.s -o m2s-$a.o
  ar rcs libs-$a.a m1-$a.o m2s-$a.o
  $cc -shared -nostdlib -Wl,-soname,libh-$a.so,-z,max-page-size=4096 -o libh-$a.so libh-$a.s
  for k in st pie; do
    case $k in
      st) w="-static"; lib=libs-$a.a;;
      pie) w="-pie -dynamic-linker $dl"; lib=lib-$a.a;;
    esac
    for l in bfd lld mold mortar; do
      case $l in
        bfd) prog=$bfd; m="";;
        lld) prog=ld.lld-19; m="";;
        mold) prog=ld.mold; m="-m $em";;
        mortar) prog=$mortar; m="";;
      esac
      [ $k-$l = pie-lld ] && continue
      # shellcheck disable=SC2086
      set -- $m $w -z max-page-size=4096 -o $k-$a.$l main-$a.o $lib
      [ $k = pie ] && set -- "$@" --as-needed libh-$a.so
      printf '%s\n' "$@" > $k-$a.$l.line
      if [ $l = mortar ]; then
        "$prog" "$@" --mortar-map=$k-$a.$l.map
      else
        "$prog" "$@" -Map=$k-$a.$l.map
      fi
    done
  done
done
