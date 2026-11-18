	.section .text.g,"axG",%progbits,g,comdat
	.weak g
	.type g,%function
g:
	mov w0, #2
	ret
	.globl hx
	.type hx,%function
hx:
	mov w0, #40
	ret
	.text
	.globl Y
	.type Y,%function
Y:
	ret
	.section .note.GNU-stack,"",%progbits
