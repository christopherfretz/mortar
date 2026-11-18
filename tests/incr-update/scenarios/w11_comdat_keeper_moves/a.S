#include "asm.h"
 .section .text.h,"axG",@progbits,G1,comdat
 .globl h
 .type h,%function
h:
 retv 2
 ret
