 .text
 .globl f
 .type f, %function
 .p2align 2
f:
 nop
 nop
 b impl
 .size f, 12
 .type impl, %function
 .p2align 2
impl:
 lsl w0, w0, 1
 ret
 .size impl, 8
