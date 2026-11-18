#!/bin/bash
# fdeglobal.sh PROBE W -- WP-238 review, item 2 (rows that change while
# the file's bytes do not). f.s (UNCHANGED) has a weak `foo` in .text.foo
# and an .eh_frame whose FDE names its code through the GLOBAL symbol foo
# (`.long foo - .`, not a section symbol) and its LSDA (.rodata.lsda, of
# f.s) through a local. mortar_merge resolves an FDE's initial location
# through the resolution (phase.rs `global`), so the FDE's code section
# is whichever file's `foo` wins. v0: g.s defines a strong foo (the FDE's
# code is g.o's .text.foo; the LSDA edge is a row of g.o's section);
# v1: g.s defines no foo (f.o's weak foo wins; the edge is f.o's).
# Updates v0 -> v1 and v1 -> v0 change only g.o. Last line FINAL.
PROBE=$1; W=$2
source "$(cd "$(dirname "$0")" && pwd)/lib.sh"
for arch in a64 x64; do
  if [ $arch = a64 ]; then CC=gcc; RA=30; RET=ret; else CC=x86_64-linux-gnu-gcc; RA=16; RET=ret; fi
  for v in 0 1; do
    d=$W/fg-$arch/v$v; mkdir -p "$d"
    printf 'int foo(void);\nint main(void){ return foo(); }\n' > "$d/m.c"
    cat > "$d/f.s" <<EOF
  .section .text.foo,"ax",%progbits
  .weak foo
  .type foo,%function
foo:
  $RET
  .size foo,.-foo
  .section .rodata.lsda,"a",%progbits
lsda:
  .long 0x4c534441
  .section .eh_frame,"a",%progbits
cie:
  .long cie_end - cie_start
cie_start:
  .long 0
  .byte 1
  .asciz "zLR"
  .uleb128 1
  .sleb128 -8
  .uleb128 $RA
  .uleb128 2
  .byte 0x1b
  .byte 0x1b
  .balign 4, 0
cie_end:
fde:
  .long fde_end - fde_start
fde_start:
  .long fde_start - cie
  .long foo - .
  .long 4
  .uleb128 4
  .long lsda - .
  .balign 4, 0
fde_end:
EOF
    if [ $v = 0 ]; then
      printf '  .section .text.foo,"ax",%%progbits\n  .globl foo\n  .type foo,%%function\nfoo:\n  %s\n  .size foo,.-foo\n' "$RET" > "$d/g.s"
    else
      printf '  .section .text.bar,"ax",%%progbits\n  .globl bar\n  .type bar,%%function\nbar:\n  %s\n  .size bar,.-bar\n' "$RET" > "$d/g.s"
    fi
    (cd "$d" && $CC -O1 -c m.c f.s g.s) || echo "compile failed"
    if [ $v = 1 ]; then cp "$W/fg-$arch/v0/m.o" "$W/fg-$arch/v0/f.o" "$d/"; fi
    eval "r$v=\$(probe_link $d $CC -static -Wl,--gc-sections m.o f.o g.o -o prog)"
    rr=$(eval echo \$r$v)
    echo "fg $arch v$v: $(sec_live "$rr" .rodata.lsda) $(sec_live "$rr" .text.foo | tr '\n' ' ')"
    echo "  replay: $(grep -h '^replay' "$d"/dump/*.sum)"
    echo "  fde rows naming .rodata.lsda: $(awk -F'\t' '$1=="F"{f[$2]=$3} $1=="S"{sf[$2]=f[$3]":"$5} $1=="S" && $5==".rodata.lsda"{l=$2} $1=="E" && $4=="S" && $5==l {print "src="sf[$2]" kind="$3}' "$rr" | tr '\n' ' ')"
    rm -f "$d/prog"
  done
  echo "fg $arch v0 -> v1 (g.o changed: its strong foo removed):"; emu "$r0" "$r1" --roots _start
  echo "fg $arch v1 -> v0 (g.o changed: a strong foo added):"; emu "$r1" "$r0" --roots _start
done
echo "FINAL fdeglobal"
