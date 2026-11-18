	.section .text.g,"axG",%progbits,g,comdat
	.weak g
	.type g,%function
g:
	movl $2, %eax
	ret
	.weak hx
	.type hx,%function
hx:
	movl $40, %eax
	ret
	.text
	.globl Y
	.type Y,%function
Y:
	subq $8, %rsp
	call A
	call hx
	addq $8, %rsp
	ret
	.section .note.GNU-stack,"",%progbits
