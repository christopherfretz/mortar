	.text
	.globl _start
	.type _start,%function
_start:
	bl Y
	bl hx
	bl g
	b .
	.section .note.GNU-stack,"",%progbits
