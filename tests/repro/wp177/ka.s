	.section .data.rel.local.DW.ref.pp,"awG",@progbits,DW.ref.pp,comdat
	.p2align 3
	.type DW.ref.pp, %object
	.size DW.ref.pp, 8
DW.ref.pp:
	.xword 0x1234
	.text
	.globl K
	.type K,%function
K:	ret
