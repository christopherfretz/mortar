#include "asm.h"
 .section .rodata.str1.8,"aMS",@progbits,1
 .balign 8
#ifndef EDIT
LA: .asciz "abc"
#else
LA: .asciz "xyz"
#endif
 .data
 .globl tab
tab:
 ptr LA
