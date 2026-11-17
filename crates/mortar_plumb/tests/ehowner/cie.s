// f's live FDE uses a CIE whose personality names the local DW.ref.pp
// of a dropped copy: the CIE is written.
	.section .data.rel.local.DW.ref.pp,"awG",%progbits,DW.ref.pp,comdat
	.p2align 3
	.type DW.ref.pp, %object
	.size DW.ref.pp, 8
DW.ref.pp:
	.8byte 0
	.text
	.globl f
	.type f, %function
f:
	.cfi_startproc
	.cfi_personality 0x9b, DW.ref.pp
	ret
	.cfi_endproc
	.size f, .-f
