#include <stdio.h>
extern int ext_var;
/* same as textrel.c but the word is in a read-only DATA section ("a"), not text */
__asm__(".section .rodata.tr,\"a\",%progbits\n.globl tr_ptr\n.p2align 3\ntr_ptr: .dc.a ext_var\n.text\n");
extern int *const tr_ptr;
int main(void) { printf("textrel_ro %d\n", *tr_ptr); return 0; }
