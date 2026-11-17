#!/bin/sh
# Regenerates WP-170 part 2's fixtures of tests/defsym.rs: --defsym=SYM=EXPR
# (spec/resolve.md §7.2, spec/plumb.md §4.4b) by mortar, lld, GNU ld and
# mold (each output with its map and, a reference's, the words its linker
# was given, `.line`), -nostdlib:
#   num  -static -e _start n.o --defsym=k=0x1234 (the one cell mold
#        links with its -Map)
#   st   -static -e _start m.o b.o --defsym=k=0x1234 --defsym=a=b
#        --defsym=c=b+4
#   ov   -static -e _start om.o b.o ov.o --defsym=a=b: the option beats
#        ov.o's a, whose own call (self) reaches b
#   mem  -static -e _start m.o b.o libam.a and st's options: the member
#        defining a (SYM) is not extracted
#   pie  -pie -dynamic-linker LD, st's options, -fPIC objects (x86-64 reads k through the GOT; AArch64
#        and x86-64 only): k is SHN_ABS and never relocated
#   sh   -shared ls.o --defsym=calla=b --defsym=k=0x10 (AArch64 and
#        x86-64 only): calla exported, preemptible by its own name
# on AArch64 (a64), x86-64 (x64) and i386 (x86). A link a linker refuses
# or crashes on (mold: B+N, and its -Map with an alias) leaves no
# fixture. The outputs are checked in.
#   sh gen.sh <mortar>
# Run from this directory. First made with the mortar of WP-170 part 2's
# base (f2213848).
mortar=$1
for a in a64 x64 x86; do
  case $a in
    a64) cc=aarch64-linux-gnu-gcc; ar=aarch64-linux-gnu-ar; bfd=aarch64-linux-gnu-ld.bfd; ld=/lib/ld-linux-aarch64.so.1; em=aarch64linux;;
    x64) cc=x86_64-linux-gnu-gcc; ar=x86_64-linux-gnu-ar; bfd=x86_64-linux-gnu-ld.bfd; ld=/lib64/ld-linux-x86-64.so.2; em=elf_x86_64;;
    x86) cc=i686-linux-gnu-gcc; ar=i686-linux-gnu-ar; bfd=i686-linux-gnu-ld.bfd; ld=/lib/ld-linux.so.2; em=elf_i386;;
  esac
  f="-O1 -fno-inline -fno-unwind-tables -fno-asynchronous-unwind-tables -fno-stack-protector"
  for s in n m b ov om am; do
    $cc $f -fno-pic -c $s.c -o $s-$a.o || exit 1
  done
  rm -f libam-$a.a
  $ar rcs libam-$a.a am-$a.o || exit 1
  cells="num st ov mem"
  if [ $a != x86 ]; then
    $cc $f -fPIC -c m.c -o mp-$a.o || exit 1
    $cc $f -fPIC -c b.c -o bp-$a.o || exit 1
    $cc $f -fPIC -c ls.c -o ls-$a.o || exit 1
    cells="$cells pie sh"
  fi
  opts="--defsym=k=0x1234 --defsym=a=b --defsym=c=b+4"
  for c in $cells; do
    case $c in
      num) w="-static -e _start n-$a.o --defsym=k=0x1234";;
      st) w="-static -e _start m-$a.o b-$a.o $opts";;
      ov) w="-static -e _start om-$a.o b-$a.o ov-$a.o --defsym=a=b";;
      mem) w="-static -e _start m-$a.o b-$a.o libam-$a.a $opts";;
      pie) w="-pie -dynamic-linker $ld -e _start mp-$a.o bp-$a.o $opts";;
      sh) w="-shared -soname libls.so ls-$a.o --defsym=calla=b --defsym=k=0x10";;
    esac
    # shellcheck disable=SC2086
    "$mortar" -m $em -z max-page-size=4096 -o $c-$a $w --mortar-map=$c-$a.map || exit 1
    for l in lld bfd mold; do
      case $l in
        lld) prog=ld.lld-19;;
        bfd) prog=$bfd;;
        mold) prog=ld.mold;;
      esac
      # shellcheck disable=SC2086
      set -- -m $em -z max-page-size=4096 -o $c-$a.$l $w -Map=$c-$a.$l.map
      if $prog "$@" 2>$c-$a.$l.err; then
        printf '%s\n' "$@" > $c-$a.$l.line
      else
        echo "$c-$a: $l fails: $(head -1 $c-$a.$l.err)"
        rm -f $c-$a.$l $c-$a.$l.map
      fi
      rm -f $c-$a.$l.err
    done
  done
done
