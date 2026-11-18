	.section .text.g,"axG",%progbits,g,comdat
	.weak g
	.type g,%function
g:
	movl $2, %eax
	ret
	.globl hx
	.type hx,%function
hx:
	movl $40, %eax
	ret
	.text
	.globl Y
	.type Y,%function
Y:
	ret
	.section .note.GNU-stack,"",%progbits
