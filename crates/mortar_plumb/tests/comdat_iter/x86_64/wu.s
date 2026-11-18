	.text
	.globl _start
	.type _start,%function
_start:
	call Y
	call hx
	call g
	jmp .
	.section .note.GNU-stack,"",%progbits
