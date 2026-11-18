	.section .text.g,"axG",%progbits,g,comdat
	.weak g
	.type g,%function
g:
	mov w0, #3
	ret
	.text
	.globl hx
	.type hx,%function
hx:
	mov w0, #7
	ret
	.section .note.GNU-stack,"",%progbits
