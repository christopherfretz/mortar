 .text
 .globl f
 .type f, @gnu_indirect_function
 .p2align 4
f:
 lea impl(%rip), %rax
 ret
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
