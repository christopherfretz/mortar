	.section .text.g,"axG",%progbits,g,comdat
	.weak g
	.type g,%function
g:
	mov w0, #2
	ret
	.weak hx
	.type hx,%function
hx:
	mov w0, #40
	ret
	.text
	.globl Y
	.type Y,%function
Y:
	stp x29, x30, [sp, -16]!
	bl hx
	ldp x29, x30, [sp], 16
	ret
	.section .note.GNU-stack,"",%progbits
