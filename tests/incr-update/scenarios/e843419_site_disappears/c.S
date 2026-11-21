#include "asm.h"
// f's ADRP at page offset 0xff8 when !defined(A) (an 843419 site: W2 a
// load, F at Q+8 a load from x0), else at 0xff0; the section is the
// same size in both versions.
 .section .text.f,"ax",@progbits
 .balign 4096
#if !defined(A)
 .skip 0xff8
#else
 .skip 0xff0
#endif
 .globl f
 .type f,%function
f:
 adrp x0, val
 ldr x2, [sp]
 ldr x0, [x0, #:lo12:val]
 ret
#if !(!defined(A))
 nop
 nop
#endif
