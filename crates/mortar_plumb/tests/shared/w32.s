/* W32: AArch64 PC-relative forms against a preemptible key. */
	.data
	.p2align 3
	.globl	w32_g
	.type	w32_g, %object
	.size	w32_g, 8
w32_g:	.xword	1
	.text
	.globl	w32_f
	.type	w32_f, %function
w32_f:	adr	x0, w32_g
	ldr	x1, w32_g
	ret
	.size	w32_f, .-w32_f
	.section .data.rel.ro,"aw"
	.p2align 2
w32_d:	.word	w32_g - .
