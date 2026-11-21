	.section .text.g,"axG",@progbits,g,comdat
	.globl g
	.type g,@function
g:
	.globl h
	.type h,@function
h:
	movl $1, %eax
	ret
	.text
	.globl f1
	.type f1,@function
f1:
	ret
