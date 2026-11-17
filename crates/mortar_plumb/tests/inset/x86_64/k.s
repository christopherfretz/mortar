	.text
	.type	kc, @function
kc:
	ret
	.size	kc, .-kc
	.globl	k_fn
	.type	k_fn, @function
k_fn:
	movl	$5, %eax
	ret
	.size	k_fn, .-k_fn
	.section	.init_array,"aw"
	.p2align	3
	.quad	kc
