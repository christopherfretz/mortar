#include "asm.h"
#ifndef EDIT
 .section .text.h,"axG",@progbits,G1,comdat
#else
 .section .text.h,"axG",@progbits,G2,comdat
#endif
#ifndef EDIT
 .globl h
#else
 .weak h
#endif
 .type h,%function
h:
 retv 2
 ret
