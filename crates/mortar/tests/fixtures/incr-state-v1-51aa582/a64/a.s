	.text
	.globl _start
	.type _start, %function
_start:
	.cfi_startproc
	adrp x1, :got:word
	ldr x1, [x1, :got_lo12:word]
	adrp x2, :got:word2
	ldr x2, [x2, :got_lo12:word2]
	bl sfn
	mov x8, #93
	svc #0
	.cfi_endproc
	.type sfn, %function
sfn:
	.cfi_startproc
	mov x0, #42
	ret
	.cfi_endproc
	.data
	.globl word
word:	.quad 1
	.globl word2
word2:	.quad 2
	.section .rodata.str1.1,"aMS",%progbits,1
msg:	.asciz "hello"
m2:	.asciz "lo"
m3:	.asciz "world"
m4:	.asciz "again"
	.section .rodata
	.quad msg
	.quad m3
	.quad m4
	.section .note.GNU-stack,"",%progbits
