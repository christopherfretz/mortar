	.text
	.globl _start
_start:
	movq dv(%rip), %rax
	jmp .
