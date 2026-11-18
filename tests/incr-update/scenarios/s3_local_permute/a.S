#include "asm.h"
// getf returns *la - *lb through the GOT. A: the locals swapped in name.
#ifndef A
#define LA la
#define LB lb
#define VA 7
#define VB 3
#else
#define LA lb
#define LB la
#define VA 3
#define VB 7
#endif
 .text
 .globl getf
 .type getf,%function
getf:
#if defined(__aarch64__)
 adrp x0, :got:LA
 ldr x0, [x0, :got_lo12:LA]
 adrp x1, :got:LB
 ldr x1, [x1, :got_lo12:LB]
 ldr w0, [x0]
 ldr w1, [x1]
 sub w0, w0, w1
#else
 movq LA@GOTPCREL(%rip), %rax
 movq LB@GOTPCREL(%rip), %rcx
 movl (%rax), %eax
 subl (%rcx), %eax
#endif
 ret
 .section .data.LA,"aw",@progbits
LA: .long VA
 .section .data.LB,"aw",@progbits
LB: .long VB
