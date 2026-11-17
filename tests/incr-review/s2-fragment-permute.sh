#!/bin/bash
# S2: a file whose string fragments are permuted by an update (same size)
# keeps its stale FRAG_AT rows; the next update, of ANOTHER file whose
# string deduplicates into the first file's fragment, maps it by position.
# usage: s2-fragment-permute.sh EMPTY-DIR   (AArch64 host)
set -u
here=$(cd "$(dirname "$0")" && pwd); . "$here/lib.sh"
cd "$1" || exit 1
as_a() { # $1 $2: the two strings in section order
cat <<A
 .section .rodata.str1.1,"aMS",@progbits,1
L1: .asciz "$1"
L2: .asciz "$2"
 .data
 .globl tab
tab: .xword L1, L2
A
}
cs() { cat <<C
 .section .rodata.str1.1,"aMS",@progbits,1
Lfoo: .asciz "foo"
 .text
 .globl _start
_start:
 mov x0,#1
 adrp x1, Lfoo
 add x1, x1, :lo12:Lfoo
 mov x2,#3
 mov x8,#64
 svc #0
 mov x0,#$1
 mov x8,#93
 svc #0
C
}
as_a foo bar > a1.s; as_a bar foo > a2.s; cs 10 > c1.s; cs 20 > c2.s
asm a1.s a.o; asm c1.s c.o
L="a.o c.o -static"
full out $L || exit 1
echo "link 0: $(runx out)   (want foo, 10)"
s0=$(stat -c %s a.o); asm a2.s a.o; echo "a.o size $s0 -> $(stat -c %s a.o)"
echo "update 1 (a.o changed): $(upd out $L)"
echo "after update 1: $(runx out)   (want foo, 10)"
full ref1 $L --mortar-map=ref1.map; echo "ref1: $(runx ref1)"; same out ref1
asm c2.s c.o
echo "update 2 (c.o changed): $(upd out $L)"
echo "after update 2: $(runx out)   (want foo, 20; bar = stale FRAG_AT)"
full ref2 $L --mortar-map=ref2.map; echo "ref2: $(runx ref2)"; same out ref2
