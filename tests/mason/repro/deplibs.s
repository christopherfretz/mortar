	.section .deplibs,"MS",@llvm_dependent_libraries,1
	.asciz "pthread"
	.text
	.globl _start
_start:
	mov x0, #0
	mov x8, #93
	svc #0
