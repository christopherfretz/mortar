	.text
	.globl _start
_start:
	adrp x0, dv
	ldr x0, [x0, :lo12:dv]
	b .
