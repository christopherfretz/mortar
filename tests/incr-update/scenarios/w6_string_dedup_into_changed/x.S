#include "asm.h"
 .section .rodata.str1.1,"aMS",@progbits,1
#ifndef EDIT
Lx: .asciz "zzz"
#else
Lx: .asciz "foo"
#endif
 .data
 ptr Lx
