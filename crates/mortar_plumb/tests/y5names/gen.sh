#!/bin/sh
# Regenerates WP-210's fixtures of tests/y5names.rs (spec/plumb.md §6.5
# Y5's closed set), AArch64 and x86-64, -nostdlib:
#   st-<arch>.<linker>   -static -T s.ld st.o        (bfd lld; mold reads
#                        no SECTIONS): names the script assigns
#   cp-<arch>.<linker>   -no-pie st.o libd.so        (bfd lld mold): dv
#                        copied into the executable, a name of .symtab
#                        and .dynsym no input defines
# Each output has its map and the words its linker was given (`.line`).
# The outputs are checked in.
#   sh gen.sh
# Run from this directory.
set -e
for a in a64 x64; do
  case $a in
    a64) cc=aarch64-linux-gnu-gcc; bfd=aarch64-linux-gnu-ld.bfd; em=aarch64linux
         dl=/lib/ld-linux-aarch64.so.1;;
    x64) cc=x86_64-linux-gnu-gcc; bfd=x86_64-linux-gnu-ld.bfd; em=elf_x86_64
         dl=/lib64/ld-linux-x86-64.so.2;;
  esac
  $cc -c st-$a.s -o st-$a.o
  $cc -c cp-$a.s -o cp-$a.o
  $cc -shared -nostdlib -Wl,-soname,libd-$a.so,-z,max-page-size=4096 -o libd-$a.so libd.s
  for l in bfd lld mold; do
    case $l in
      bfd) prog=$bfd; m="";;
      lld) prog=ld.lld-19; m="";;
      mold) prog=ld.mold; m="-m $em";;
    esac
    if [ $l != mold ]; then
      # shellcheck disable=SC2086
      set -- $m -static -T s.ld -o st-$a.$l st-$a.o
      printf '%s\n' "$@" > st-$a.$l.line
      $prog "$@" -Map=st-$a.$l.map
    fi
    # shellcheck disable=SC2086
    set -- $m -no-pie -dynamic-linker $dl -z max-page-size=4096 -o cp-$a.$l cp-$a.o libd-$a.so
    printf '%s\n' "$@" > cp-$a.$l.line
    $prog "$@" -Map=cp-$a.$l.map
  done
done
