	.section .text.g,"axG",%progbits,g,comdat
	.globl g
	.type g,%function
g:
	mov w0, #2
	ret
	.text
	.globl f2
	.type f2,%function
f2:
	bl f1
	bl g
	ret
