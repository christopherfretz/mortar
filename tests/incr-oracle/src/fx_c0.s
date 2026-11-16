 .text
 .globl f
 .type f, @function
 .p2align 4
f:
 nop
 nop
 jmp impl
 nop
 nop
 nop
 nop
 nop
 .size f, 12
 .type impl, @function
 .p2align 4
impl:
 lea (%rdi,%rdi), %eax
 ret
 .size impl, 4
