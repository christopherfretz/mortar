	.text
	.globl _start
	.type _start, @function
_start:
	.cfi_startproc
	movq word@GOTPCREL(%rip), %rax
	leaq msg(%rip), %rsi
	call sfn
	movl $60, %eax
	syscall
	.cfi_endproc
	.type sfn, @function
sfn:
	.cfi_startproc
	movl $42, %edi
	ret
	.cfi_endproc
	.data
	.globl word
word:	.quad 1
	.section .rodata.str1.1,"aMS",@progbits,1
msg:	.asciz "hello"
m2:	.asciz "lo"
m3:	.asciz "world"
m4:	.asciz "again"
	.section .rodata
	.long 7
	.quad m3
	.quad m4
	.section .note.GNU-stack,"",@progbits
