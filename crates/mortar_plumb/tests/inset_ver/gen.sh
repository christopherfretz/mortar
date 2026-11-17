#!/bin/sh
# Regenerates the fixtures of tests/inset_ver.rs (resolve §9.2 in the
# input set, spec/plumb.md §3.8): a shared object's undefined reference
# that requires a version (`g@V1`) has the key `g@V1`, which an archive
# member's plain `g` does not define; an unversioned one (`g`) is an edge
# to it. For AArch64 and x86-64:
#   libv.so  defines g@@V1;  libu.so refers to g@V1 (linked against
#   libv.so);  libw.so refers to g (linked against nothing);  libg.a(g.o)
#   defines plain g.
#   pv, pv.map / pv.gnu, pv.gnu.map / pv.lld, pv.lld.map:
#       m.o libu.so libg.a -rpath-link .   (g.o not extracted: g@V1 is
#       libv.so's, a closure file)
#   pv.mold, pv.mold.map: the same (mold extracts g.o by the plain name:
#       spec/plumb.md §10.2 A49)
#   pw, pw.map / pw.gnu, pw.gnu.map / pw.lld, pw.lld.map, pw.mold, pw.mold.map:
#       m.o libw.so libg.a                 (g.o extracted)
#   sh gen.sh <mortar>   (GNU ld, ld.lld and mold 2.37.1 from PATH)
# Run from this directory.
set -e
mortar=$1
for arch in aarch64 x86_64; do
  mkdir -p "$arch"
  cd "$arch"
  for s in m v u w g; do
    "$arch-linux-gnu-as" "../$s.s" -o "$s.o"
  done
  ld="$arch-linux-gnu-ld"
  "$ld" -shared -soname libv.so --version-script ../v.ver v.o -o libv.so
  "$ld" -shared -soname libu.so u.o libv.so -o libu.so
  "$ld" -shared -soname libw.so w.o -o libw.so
  rm -f libg.a
  "$arch-linux-gnu-ar" rcsD libg.a g.o
  dl=/lib/ld-linux-aarch64.so.1 emu=aarch64linux
  [ "$arch" = x86_64 ] && dl=/lib64/ld-linux-x86-64.so.2 emu=elf_x86_64
  for p in v w; do
    lib=libu.so
    [ $p = w ] && lib=libw.so
    line="-dynamic-linker $dl m.o $lib libg.a -rpath-link ."
    "$mortar" $line -o p$p --mortar-map=p$p.map
    "$ld" $line -o p$p.gnu -Map=p$p.gnu.map
    ld.lld $line -o p$p.lld -Map=p$p.lld.map
    mold -m $emu $line -o p$p.mold -Map=p$p.mold.map
  done
  rm -f v.o u.o w.o
  cd ..
done
