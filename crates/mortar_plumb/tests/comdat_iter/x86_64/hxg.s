	.section .text.g,"axG",%progbits,g,comdat
	.weak g
	.type g,%function
g:
	movl $3, %eax
	ret
	.text
	.globl hx
	.type hx,%function
hx:
	movl $7, %eax
	ret
	.section .note.GNU-stack,"",%progbits
