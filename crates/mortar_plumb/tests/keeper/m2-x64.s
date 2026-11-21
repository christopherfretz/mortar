	.section .text.g,"axG",@progbits,g,comdat
	.globl g
	.type g,@function
g:
	movl $2, %eax
	ret
	.text
	.globl f2
	.type f2,@function
f2:
	call f1
	call g
	call h@PLT
	ret
