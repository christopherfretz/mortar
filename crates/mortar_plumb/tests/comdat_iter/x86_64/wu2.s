	.text
	.globl _start
	.type _start,%function
_start:
	call Y
	call g
	jmp .
	.section .note.GNU-stack,"",%progbits
