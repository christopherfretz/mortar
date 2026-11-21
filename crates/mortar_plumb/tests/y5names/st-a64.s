	.text
	.globl _start
_start:
	adrp x0, :got:my_abs
	ldr x0, [x0, :got_lo12:my_abs]
	adrp x0, :got:my_end
	ldr x0, [x0, :got_lo12:my_end]
	b .
	.weak my_abs, my_end
	.data
	.quad 7
