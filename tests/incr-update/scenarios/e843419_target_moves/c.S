#include "asm.h"
// f's ADRP at page offset 0xff8: an 843419 site (W2 a load, F at Q+8 a
// load from x0), whose patch holds W4' (`ldr x0, [x0, #:lo12:val]`).
 .section .text.f,"ax",@progbits
 .balign 4096
 .skip 0xff8
 .globl f
 .type f,%function
f:
 adrp x0, val
 ldr x2, [sp]
 ldr x0, [x0, #:lo12:val]
 ret
