// fx's live FDE reads its LSDA at xlsda, a global of tb.o's table.
	.text
	.globl fx
	.type fx, %function
fx:
	.cfi_startproc
	.cfi_lsda 0x1b, xlsda
	ret
	.cfi_endproc
	.size fx, .-fx
