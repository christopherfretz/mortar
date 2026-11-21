	.text
	.globl _start
_start:
	movq my_abs@GOTPCREL(%rip), %rax
	movq my_end@GOTPCREL(%rip), %rax
	jmp .
	.weak my_abs, my_end
	.data
	.quad 7
