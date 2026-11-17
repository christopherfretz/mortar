	.section .foo,"ae",%progbits
	.globl	gex
gex:	.long	1,2
	.section .note.gnu.build-id,"a",%note
	.p2align 2
	.globl	gbid
gbid:	.long	4,8,3
	.asciz	"GNU"
	.long	1,2
	.section .note.GNU-stack,"",%progbits
