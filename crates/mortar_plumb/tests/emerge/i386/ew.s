# Address words of `gms`, `wcs` and of em.o's local `lms` (by its own
# empty section's local here) and the section symbol.
	.section .rodata.str1.1,"aMS",%progbits,1
lms:
	.data
	.p2align 3
	.globl	gp
gp:	.dc.a	gms
	.globl	wp
wp:	.dc.a	wcs
	.globl	lp
lp:	.dc.a	lms
	.globl	sp
sp:	.dc.a	.rodata.str1.1
	.section .note.GNU-stack,"",%progbits
