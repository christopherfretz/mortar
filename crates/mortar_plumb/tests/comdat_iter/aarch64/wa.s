	.section .text.g,"axG",%progbits,g,comdat
	.weak g
	.type g,%function
g:
	mov w0, #1
	ret
	.section .note.GNU-stack,"",%progbits
