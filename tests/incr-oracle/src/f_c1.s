 .text
 .globl f
 .type f, %gnu_indirect_function
 .p2align 2
f:
 adrp x0, impl
 add x0, x0, :lo12:impl
 ret
 .size f, 12
 .type impl, %function
 .p2align 2
impl:
 lsl w0, w0, 1
 ret
 .size impl, 8
