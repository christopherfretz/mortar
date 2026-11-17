	.section .note.GNU-stack,"",%progbits
	.section .text.g,"axG",%progbits,g,comdat
	.weak g
	.type g,%function
g:
#if defined(__aarch64__)
	mov w0, #1
#else
	movl $1, %eax
#endif
	ret
