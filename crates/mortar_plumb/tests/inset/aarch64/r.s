	.text
	.globl	_start
	.type	_start, %function
_start:
	mov	x0, #0
	mov	x8, #93
	svc	#0
	.size	_start, .-_start
	.data
	.xword	k_fn
