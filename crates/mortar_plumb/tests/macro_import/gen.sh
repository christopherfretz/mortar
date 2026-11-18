#!/bin/sh
# Regenerates the fixtures of tests/macro_import.rs (WP-250, spec/image.md
# I-47a): mortar's static links, AArch64 and x86-64, of
#   gcc/<arch>/dw<v>/   GCC 14 (the triple's gcc) -O0 -g3 -gdwarf-<v> objects of src/ (a b c:
#                       b's and c's .debug_macro groups are dropped copies,
#                       each imported by DW_MACRO_import), linked -e main
#                       (no library: never run); imports.txt: per import of
#                       a dropped copy, the output's file offset, the width
#                       and the kept copy's offset (+ addend), from WP-249's
#                       independent reader tests/image-design/wp249/macros.py
#                       (its 'redirect', which gdb and lldb read right).
#   hand/<arch>/        hand/k.s (first: the keeper) and hand/d.s, one group
#                       per branch of I-47a's condition, --gc-sections.
#   hand2/<arch>/       hand2/k.s and hand2/d.s: the supervisor's amendment
#                       (K and D placed whole: SHF_MERGE members) and a
#                       .debug_info member referred to from
#                       .debug_gnu_pubnames. (hand2/r.s is linked by the
#                       test itself, with lld: mortar refuses it.)
# mortar at the time wrote I-47's all-ones tombstone in every such field
# (before WP-249 step 2); the tests patch the fields themselves.
#   sh gen.sh <mortar>          (run from this directory)
set -e
mortar=$1
here=$(pwd)
mp=$here/../../../../tests/image-design/wp249/macros.py
for arch in aarch64 x86_64; do
  case $arch in aarch64) em=aarch64linux;; x86_64) em=elf_x86_64;; esac
  for v in 4 5; do
    d=gcc/$arch/dw$v
    mkdir -p "$d"
    for f in a b c; do
      (cd src && "$arch-linux-gnu-gcc" -O0 -g3 -gdwarf-$v -ffile-prefix-map="$here/src"=. \
        -fno-asynchronous-unwind-tables -c "$f.c" -o "../$d/$f.o")
    done
    (cd "$d" && "$mortar" -m $em -static --build-id=none -e main a.o b.o c.o -o out --mortar-map=out.map)
    python3 -I "$mp" patch "$d/out" redirect.tmp redirect "$d/a.o" "$d/b.o" "$d/c.o" > /dev/null
    python3 -I -c '
import sys
a = open(sys.argv[1], "rb").read(); b = open(sys.argv[2], "rb").read()
assert len(a) == len(b)
i = 0
while i < len(a):
    # an import field: 4 bytes (32-bit DWARF), all ones in the output
    if a[i:i + 4] == b"\xff" * 4 and b[i:i + 4] != a[i:i + 4]:
        print("%#x 4 %#x" % (i, int.from_bytes(b[i:i + 4], "little")))
        i += 4
    else:
        assert a[i] == b[i], hex(i)
        i += 1
' "$d/out" redirect.tmp > "$d/imports.txt"
    rm -f redirect.tmp
  done
  h=hand/$arch
  mkdir -p "$h"
  "$arch-linux-gnu-as" hand/k.s -o "$h/k.o"
  "$arch-linux-gnu-as" hand/d.s -o "$h/d.o"
  (cd "$h" && "$mortar" -m $em -static --gc-sections --build-id=none k.o d.o -o out --mortar-map=out.map)
  h=hand2/$arch
  mkdir -p "$h"
  "$arch-linux-gnu-as" hand2/k.s -o "$h/k.o"
  "$arch-linux-gnu-as" hand2/d.s -o "$h/d.o"
  (cd "$h" && "$mortar" -m $em -static --gc-sections --build-id=none k.o d.o -o out --mortar-map=out.map)
done
