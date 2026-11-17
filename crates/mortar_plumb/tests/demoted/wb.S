	.text
	.globl Y
	.type Y,%function
Y:
#if defined(__aarch64__)
	stp x29, x30, [sp, -16]!
	bl hx
	ldp x29, x30, [sp], 16
#else
	subq $8, %rsp
	call hx
	addq $8, %rsp
#endif
	ret
	.section .text.g,"axG",%progbits,g,comdat
	.weak g
	.type g,%function
g:
#if defined(__aarch64__)
	mov w0, #2
#else
	movl $2, %eax
#endif
	ret
#ifndef UNDEF
	.weak hx
	.type hx,%function
hx:
#if defined(__aarch64__)
	mov w0, #40
#else
	movl $40, %eax
#endif
	ret
#else
	.weak hx
#endif
	.section .note.GNU-stack,"",%progbits
