	.text
	.globl	j_fn
	.type	j_fn, @function
j_fn:
	movl	$7, %eax
	ret
	.size	j_fn, .-j_fn
