	.text
	.globl	f
	.type	f,%function
f:	ret
	.data
	.globl	gplain
	.type	gplain,%object
	.size	gplain,4
gplain:	.long	7
	.section .note.GNU-stack,"",%progbits
