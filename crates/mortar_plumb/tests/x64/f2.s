# W8: a .init fragment aligned above crti's end: the gap runs at start-up.
	.text
	.p2align 5
	.globl a2
a2:	ret
	.section .init,"ax",@progbits
	.p2align 4
	nop
	.section .note.GNU-stack,"",@progbits
