# An empty merge section after a merged input (layout.md L-162: at the
# end of the merged chunk).
	.section .rodata.str1.1,"aMS",%progbits,1
	.globl	gmid
gmid:
	.data
	.p2align 3
	.globl	mp
mp:	.dc.a	gmid
	.section .note.GNU-stack,"",%progbits
