#!/bin/bash
# S4: one hop from a full link's state. A string present in two merge
# classes of one output section (.rodata.str1.1 and .rodata.str1.8) has a
# FRAGS identity with ordinals; the changed file drops its copy. The
# positional replay takes the unchanged file's fragment from FRAG_AT, the
# identity replay ranks it again: do they agree?
# usage: s4-frag-ordinals.sh EMPTY-DIR   (AArch64 host)
set -u
here=$(cd "$(dirname "$0")" && pwd); . "$here/lib.sh"
cd "$1" || exit 1
as_a() { cat <<A
 .section .rodata.str1.8,"aMS",@progbits,1
 .balign 8
LA: .asciz "$1"
 .data
 .globl tab
tab: .xword LA
A
}
cat > b.s <<'B'
 .section .rodata.str1.1,"aMS",@progbits,1
Labc: .asciz "abc"
 .section .rodata.str1.8,"aMS",@progbits,1
 .balign 8
Lxyz: .asciz "xyz"
 .text
 .globl _start
_start:
 mov x0,#1
 adrp x1, Labc
 add x1, x1, :lo12:Labc
 mov x2,#3
 mov x8,#64
 svc #0
 mov x0,#1
 adrp x1, Lxyz
 add x1, x1, :lo12:Lxyz
 mov x2,#3
 mov x8,#64
 svc #0
 mov x0,#0
 mov x8,#93
 svc #0
B
as_a abc > a1.s; as_a xyz > a2.s
asm a1.s a.o; asm b.s b.o
L="a.o b.o -static"
full out $L --mortar-map=out0.map || exit 1
echo "link 0: $(runx out)   (want abcxyz)"
cp out out0
s0=$(stat -c %s a.o); asm a2.s a.o; echo "a.o size $s0 -> $(stat -c %s a.o)"
echo "update 1 (a.o changed): $(upd out $L)"
echo "after update 1: $(runx out)"
full ref1 $L --mortar-map=ref1.map; echo "ref1: $(runx ref1)"; same out ref1
