#include "asm.h"
// A: the two strings in the other order (same size).
 .section .rodata.str1.1,"aMS",@progbits,1
#ifndef A
L1: .asciz "foo"
L2: .asciz "bar"
#else
L2: .asciz "bar"
L1: .asciz "foo"
#endif
 .data
 .globl tab
tab:
 ptr L1
 ptr L2
