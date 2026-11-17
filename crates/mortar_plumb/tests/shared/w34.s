/* W34: x86-64 PC32 data words against a preemptible key (in its own
   section, so that the assembler leaves the difference to the linker)
   and an imported one. */
	.section .data.g,"aw"
	.p2align 2
	.globl	w34_g
	.type	w34_g, @object
	.size	w34_g, 4
w34_g:	.long	1
	.data
	.p2align 2
	.globl	w34_d
w34_d:	.long	w34_g - .
	.long	w34_ext - .
