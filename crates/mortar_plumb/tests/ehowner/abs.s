// fa's live FDE has an LSDA pointer against an absolute symbol: the
// verifier cannot place its target in an input section, so the start
// may be in any table and no owner is named anywhere (the dead word of
// dead.o included).
	.text
	.globl fa
	.type fa, %function
fa:
	.cfi_startproc
	.cfi_lsda 0x1b, xabs
	ret
	.cfi_endproc
	.size fa, .-fa
	.globl xabs
	.set xabs, 0x40
