#!/bin/bash
# S3: a file whose LOCAL symbols are permuted by an update (same size)
# keeps stale SYMBOLS at_index rows; the next update of another file maps
# the GOT slots of those locals by position (no check on locals).
# Run with MORTAR_FLOOR_WRITE_WINDOWS=1 to see the in-memory image.
# usage: s3-local-permute.sh EMPTY-DIR   (AArch64 host)
set -u
here=$(cd "$(dirname "$0")" && pwd); . "$here/lib.sh"
cd "$1" || exit 1
as_a() { # $1 $2: the local mentioned first, then second
cat <<A
 .text
 .globl getf
 .type getf,%function
getf:
 adrp x0, :got:$1
 ldr x0, [x0, :got_lo12:$1]
 adrp x1, :got:$2
 ldr x1, [x1, :got_lo12:$2]
 ldr w0, [x0]
 ldr w1, [x1]
 sub w0, w0, w1
 ret
 .section .data.$1,"aw",@progbits
$1: .word $3
 .section .data.$2,"aw",@progbits
$2: .word $4
A
}
cs() { cat <<C
 .text
 .globl _start
_start:
 bl getf
 add w0,w0,#$1
 mov x8,#93
 svc #0
C
}
# getf returns la-lb (=4) in a1 and lb-la with lb=3, la=7 swapped... see values.
as_a la lb 7 3 > a1.s; as_a lb la 3 7 > a2.s; cs 10 > c1.s; cs 20 > c2.s
asm a1.s a.o; asm c1.s c.o
L="a.o c.o -static"
full out $L || exit 1
echo "link 0: $(runx out)   (want 14)"
readelf -sW a.o | grep -E 'SECTION|l[ab]$'
s0=$(stat -c %s a.o); asm a2.s a.o; echo "a.o size $s0 -> $(stat -c %s a.o)"
readelf -sW a.o | grep -E 'SECTION|l[ab]$'
echo "update 1 (a.o changed): $(upd out $L)"
echo "after update 1: $(runx out)   (want 6)"
asm c2.s c.o
echo "update 2 (c.o changed): $(upd out $L)"
echo "after update 2: $(runx out)   (want 16)"
full ref2 $L; echo "ref2: $(runx ref2)"; same out ref2
