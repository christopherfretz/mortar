// Two FDEs read one LSDA: g's (a dropped copy: dead) and f3's (live).
// The owner is live.
	.section .data.rel.local.DW.ref.pp,"awG",%progbits,DW.ref.pp,comdat
	.p2align 3
	.type DW.ref.pp, %object
	.size DW.ref.pp, 8
DW.ref.pp:
	.8byte 0
	.section .text.g,"axG",%progbits,g,comdat
	.weak g
	.type g, %function
g:
	.cfi_startproc
	.cfi_lsda 0x1b, .LLSDA
	ret
	.cfi_endproc
	.size g, .-g
	.text
	.globl f3
	.type f3, %function
f3:
	.cfi_startproc
	.cfi_lsda 0x1b, .LLSDA
	ret
	.cfi_endproc
	.size f3, .-f3
	.section .gcc_except_table,"a",%progbits
	.p2align 4
.LLSDA:
	.4byte 0x11223344
	.4byte DW.ref.pp-.
	.4byte 0x55667788
	.4byte 0
