#!/bin/bash
# Hostile states against the positional replay: one program (a.o: two
# function sections f, g; two merged strings foo, bar; c.o: calls f,
# prints foo, exits with f()+K), its full link's state, c.o edited, then
# the state rewritten by `state_edit OP ARGS` (hashes recomputed) and the
# test path's update run (by position; MORTAR_FLOOR_IDENTITY=1: by identity).
# usage: h-hostile.sh EMPTY-DIR OP [ARGS...]     (or OP=dump)
# Needs STATE_EDIT (built by build-state-edit.sh).
set -u
here=$(cd "$(dirname "$0")" && pwd); . "$here/lib.sh"
dir=$1; shift
cd "$dir" || exit 1
V=0.0.0
cat > a.s <<'A'
 .section .text.f,"ax",@progbits
 .globl f
 .type f,%function
f: mov w0,#1
 ret
 .section .text.g,"ax",@progbits
 .globl g
 .type g,%function
g: mov w0,#2
 ret
 .section .rodata.str1.1,"aMS",@progbits,1
L1: .asciz "foo"
L2: .asciz "bar"
 .data
 .globl tab
tab: .xword L1, L2
A
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
 bl f
 add w0,w0,#$1
 mov x8,#93
 svc #0
C
}
cs 10 > c1.s; cs 20 > c2.s
asm a.s a.o; asm c1.s c.o
L="a.o c.o -static"
full out $L >/dev/null 2>&1 || { echo "link 0 failed"; exit 1; }
if [ "$1" = dump ]; then "$STATE_EDIT" out.mortar-state $V dump; exit 0; fi
asm c2.s c.o
full ref $L; echo "ref: $(runx ref)"
"$STATE_EDIT" out.mortar-state $V "$@" || exit 1
echo "update ($([ -n "${MORTAR_FLOOR_IDENTITY:-}" ] && echo identity || echo position)): $(upd out $L)"
echo "after: $(runx out)   (ref: foo, 21)"
same out ref
