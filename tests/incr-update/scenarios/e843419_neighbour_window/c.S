#include "asm.h"
// f is c1's last two words and c2: with A the ADRP is c1's LAST word, at
// page offset 0xff8, and the window's W2 and F are c2's first words (the
// window straddles the neighbour section c2, which neither changes nor
// moves: c1 keeps its size); without A the ADRP is at 0xff4 (no site).
 .section .text.c1,"ax",@progbits
 .balign 4096
 .skip 0xff4
 .globl f
 .type f,%function
f:
#ifdef A
 nop
 adrp x0, val
#else
 adrp x0, val
 nop
#endif
 .section .text.c2,"ax",@progbits
 .balign 4
 ldr x2, [sp]
 ldr x0, [x0, #:lo12:val]
 ret
