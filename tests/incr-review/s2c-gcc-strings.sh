#!/bin/bash
# S2 with gcc -O2 objects: a.c's edit swaps two calls (a realistic edit);
# both strings 13 bytes; gcc emits .rodata.str1.1 in the new order at the same object size. c.c
# shares a string literal with a.c. Update 1 = a.o; update 2 = c.o.
# usage: s2c-gcc-strings.sh EMPTY-DIR    (AArch64 host)
set -u
here=$(cd "$(dirname "$0")" && pwd); . "$here/lib.sh"
cd "$1" || exit 1
cc1() { gcc -O2 -ffreestanding -fno-pic -fno-asynchronous-unwind-tables -c "$1" -o "$2.new" && mv -f "$2.new" "$2"; }
cat > a1.c <<'C'
extern void put(const char *);
void pa(void) { put("hello from A\n"); put("shared line!\n"); }
C
cat > a2.c <<'C'
extern void put(const char *);
void pa(void) { put("shared line!\n"); put("hello from A\n"); }
C
k() { printf 'extern void put(const char *);\nint k(void) { put("shared line!\\n"); return %s; }\n' "$1"; }
k 10 > c1.c; k 20 > c2.c
cat > rt.s <<'S'
 .text
 .globl put
put: mov x1, x0
 mov x2, #0
1: ldrb w3, [x1, x2]
 cbz w3, 2f
 add x2, x2, #1
 b 1b
2: mov x0, #1
 mov x8, #64
 svc #0
 ret
 .globl _start
_start:
 bl k
 mov x8, #93
 svc #0
S
asm rt.s rt.o; cc1 a1.c a.o; cc1 c1.c c.o
L="a.o c.o rt.o -static"
full out $L || exit 1
echo "link 0: $(runx out)"
s0=$(stat -c %s a.o); cc1 a2.c a.o; echo "a.o size $s0 -> $(stat -c %s a.o)"
readelf -p .rodata.str1.8 -p .rodata.str1.1 a.o 2>/dev/null | grep '\]'
echo "update 1 (a.o): $(upd out $L | sed 's/ windows=.*//')"
echo "after update 1: $(runx out)"
cc1 c2.c c.o
echo "update 2 (c.o): $(upd out $L | sed 's/ windows=.*//')"
echo "after update 2: $(runx out)   (want 'shared line!', 20)"
full ref2 $L; echo "ref2: $(runx ref2)"
