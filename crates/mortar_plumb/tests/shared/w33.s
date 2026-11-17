/* W33: an AArch64 PREL32 data word against a preemptible key and one
   against a weak undefined key. `w33_g` is in its own section so that
   the assembler leaves the difference to the linker. */
	.section .data.g,"aw"
	.p2align 2
	.globl	w33_g
	.type	w33_g, %object
	.size	w33_g, 4
w33_g:	.word	1
	.data
	.p2align 2
	.weak	w33_wu
	.globl	w33_d
w33_d:	.word	w33_g - .
	.word	w33_wu - .
