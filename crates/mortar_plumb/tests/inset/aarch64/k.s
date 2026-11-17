	.text
	.type	kc, %function
kc:
	ret
	.size	kc, .-kc
	.globl	k_fn
	.type	k_fn, %function
k_fn:
	mov	w0, #5
	ret
	.size	k_fn, .-k_fn
	.section	.init_array,"aw"
	.p2align	3
	.xword	kc
