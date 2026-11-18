	.section .text.g,"axG",%progbits,g,comdat
	.weak g
	.type g,%function
g:
	movl $1, %eax
	ret
	.section .note.GNU-stack,"",%progbits
