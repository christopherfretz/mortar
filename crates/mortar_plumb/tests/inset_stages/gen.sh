#!/bin/sh
# Regenerates the fixtures of tests/inset_stages.rs (dyn.md D-12's
# stages in the input set and in N, spec/plumb.md §3.8, WP-171). For
# AArch64 and x86-64, each line linked by mortar (`<p>`, `<p>.map`), GNU
# ld, ld.lld and mold (`<p>.<linker>`, `<p>.<linker>.map`):
#   r1    m1.o --as-needed libu.so libg.a        (row 1: g.o at stage 2)
#   r1c   m0.o --as-needed libu.so libg.a        (1c: libu.so not needed;
#                                                 lld extracts g.o, A70)
#   k2a   m1y.o --as-needed libuy.so libgy.a     (2a: libuy.so kept
#                                                 needed; lld drops it, A71)
#   j2b   m1.o --as-needed libu.so libgv.a libv.so  (2b: libv.so joins
#                                                 at stage 2; D-19a order)
#   vr    m1.o --as-needed libuv.so libg.a libgver.so  (4b: g@V is no
#                                                 edge; mold extracts, A49)
#   nr    m0.o --no-as-needed libu.so libg.a     (g.o extracted; judged
#                                                 against r1c's line, a
#                                                 DT_NEEDED nothing justifies)
#   sh gen.sh <mortar>   (GNU ld, ld.lld and mold 2.37.1 from PATH)
# Run from this directory.
set -e
mortar=$1
for arch in aarch64 x86_64; do
  mkdir -p "$arch"
  cd "$arch"
  for s in m0 m1 m1y u uy g gy gv v gver; do
    "$arch-linux-gnu-as" "../$s.s" -o "$s.o"
  done
  ld="$arch-linux-gnu-ld"
  "$ld" -shared -soname libu.so u.o -o libu.so
  "$ld" -shared -soname libuy.so uy.o -o libuy.so
  "$ld" -shared -soname libv.so v.o -o libv.so
  "$ld" -shared -soname libgver.so --version-script ../gver.ver gver.o -o libgver.so
  "$ld" -shared -soname libuv.so u.o libgver.so -o libuv.so
  rm -f libg.a libgy.a libgv.a
  "$arch-linux-gnu-ar" rcsD libg.a g.o
  "$arch-linux-gnu-ar" rcsD libgy.a gy.o
  "$arch-linux-gnu-ar" rcsD libgv.a gv.o
  dl=/lib/ld-linux-aarch64.so.1 emu=aarch64linux
  [ "$arch" = x86_64 ] && dl=/lib64/ld-linux-x86-64.so.2 emu=elf_x86_64
  for p in r1 r1c k2a j2b vr nr; do
    case $p in
      r1) rest="m1.o --as-needed libu.so libg.a" ;;
      r1c) rest="m0.o --as-needed libu.so libg.a" ;;
      k2a) rest="m1y.o --as-needed libuy.so libgy.a" ;;
      j2b) rest="m1.o --as-needed libu.so libgv.a libv.so" ;;
      vr) rest="m1.o --as-needed libuv.so libg.a libgver.so" ;;
      nr) rest="m0.o --no-as-needed libu.so libg.a" ;;
    esac
    line="-dynamic-linker $dl $rest"
    "$mortar" $line -o $p --mortar-map=$p.map
    "$ld" $line -o $p.gnu -Map=$p.gnu.map
    ld.lld $line -o $p.lld -Map=$p.lld.map
    mold -m $emu $line -o $p.mold -Map=$p.mold.map
  done
  rm -f u.o uy.o v.o gver.o
  cd ..
done
