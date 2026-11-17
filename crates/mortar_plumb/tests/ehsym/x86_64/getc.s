# A function with call-frame information: a CIE and an FDE, live.
	.text
	.globl getc_fn
	.type getc_fn,@function
getc_fn:
	.cfi_startproc
	ret
	.cfi_endproc
