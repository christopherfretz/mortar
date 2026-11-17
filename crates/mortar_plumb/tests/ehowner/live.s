// f2 is live and its LSDA names the local DW.ref.pp of a dropped copy:
// the owner is live.
	.section .data.rel.local.DW.ref.pp,"awG",%progbits,DW.ref.pp,comdat
	.p2align 3
	.type DW.ref.pp, %object
	.size DW.ref.pp, 8
DW.ref.pp:
	.8byte 0
	.text
	.globl f2
	.type f2, %function
f2:
	.cfi_startproc
	.cfi_lsda 0x1b, .LLSDA
	ret
	.cfi_endproc
	.size f2, .-f2
	.section .gcc_except_table,"a",%progbits
	.p2align 4
.LLSDA:
	.4byte 0x11223344
	.4byte DW.ref.pp-.
	.4byte 0x55667788
	.4byte 0
