#!/bin/bash
# Hostile LOCAL rows: a.o loads two locals through the GOT (section
# symbols of .data.la / .data.lb), so SYMBOLS holds local rows and SLOTS
# names them. Link 0, c.o edited, then SYMBOLS row ROW's at_index set to
# V (state_edit local-index), then the update (by position; with
# MORTAR_FLOOR_IDENTITY=1 by identity). usage: h-local.sh EMPTY-DIR [ROW V | dump]
set -u
here=$(cd "$(dirname "$0")" && pwd); . "$here/lib.sh"
cd "$1" || exit 1; shift
cat > a.s <<'A'
 .text
 .globl getf
 .type getf,%function
getf:
 adrp x0, :got:la
 ldr x0, [x0, :got_lo12:la]
 adrp x1, :got:lb
 ldr x1, [x1, :got_lo12:lb]
 ldr w0, [x0]
 ldr w1, [x1]
 sub w0, w0, w1
 ret
 .section .data.la,"aw",@progbits
la: .word 7
 .section .data.lb,"aw",@progbits
lb: .word 3
A
cs() { printf ' .text\n .globl _start\n_start:\n bl getf\n add w0,w0,#%s\n mov x8,#93\n svc #0\n' "$1"; }
cs 10 > c1.s; cs 20 > c2.s
asm a.s a.o; asm c1.s c.o
L="a.o c.o -static"
full out $L >/dev/null 2>&1 || { echo "link 0 failed"; exit 1; }
if [ "$1" = dump ]; then "$STATE_EDIT" out.mortar-state 0.0.0 dump | grep -E 'SYMBOLS|SLOTS'; readelf -sW a.o | sed -n 4,12p; exit 0; fi
asm c2.s c.o
full ref $L
"$STATE_EDIT" out.mortar-state 0.0.0 local-index "$1" "$2" || exit 1
echo "update: $("$FLOOR" in-place -- $L -o out --incremental 2>&1 | grep -m1 -E 'ok |failed|panicked')"
echo "after: $(runx out)  ref: $(runx ref)"; same out ref
