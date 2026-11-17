	.text
	.globl	_start
	.type	_start, @function
_start:
	movl	$60, %eax
	xorl	%edi, %edi
	syscall
	.size	_start, .-_start
	.data
	.quad	k_fn
