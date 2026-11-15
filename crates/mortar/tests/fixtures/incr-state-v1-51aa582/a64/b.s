	.text
	.globl bfn
	.type bfn, %function
bfn:
	.cfi_startproc
	mov x0, #7
	ret
	.cfi_endproc
	.type lfn, %function
lfn:
	.cfi_startproc
	ret
	.cfi_endproc
	.section .note.GNU-stack,"",%progbits
