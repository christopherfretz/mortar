#!/bin/bash
# S1: a file whose sections are permuted by an update (same size) keeps
# its stale SECTIONS rows in the state the test path writes; the next
# update, of ANOTHER file, then maps the first file by position.
# usage: s1-section-permute.sh EMPTY-DIR   (AArch64 host)
set -u
here=$(cd "$(dirname "$0")" && pwd); . "$here/lib.sh"
cd "$1" || exit 1
fg() { # $1: first section, $2: second
cat <<A
 .section .text.$1,"ax",@progbits
 .globl $1
 .type $1,%function
$1: mov w0,#$3
 ret
 .section .text.$2,"ax",@progbits
 .globl $2
 .type $2,%function
$2: mov w0,#$4
 ret
A
}
cs() { cat <<C
 .text
 .globl _start
_start:
 bl f
 add w0,w0,#$1
 mov x8,#93
 svc #0
C
}
fg f g 1 2 > a1.s; fg g f 2 1 > a2.s; cs 10 > c1.s; cs 20 > c2.s
asm a1.s a.o; asm c1.s c.o
L="a.o c.o -static"
full out $L || exit 1
echo "link 0: $(runx out)   (f()+10: want 11)"
s0=$(stat -c %s a.o); asm a2.s a.o; echo "a.o size $s0 -> $(stat -c %s a.o)"
echo "update 1 (a.o changed): $(upd out $L)"
echo "after update 1: $(runx out)   (want 11)"
full ref1 $L; echo "ref1: $(runx ref1)"; same out ref1
asm c2.s c.o
echo "update 2 (c.o changed): $(UPD_EXTRA=--mortar-map=x2.map upd out $L)"
echo "after update 2: $(runx out)   (want 21; 22 = g was called)"
full ref2 $L --mortar-map=ref2.map; echo "ref2: $(runx ref2)"; same out ref2
[ -f x2.map ] && equiv x2.map ref2.map out ref2
