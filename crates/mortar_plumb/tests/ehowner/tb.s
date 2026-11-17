// tb.o's table: g's LSDA (g a dropped copy: its FDE dead) at 0, then
// xlsda, fx's (x.o's live FDE), whose word names the local DW.ref.pp.
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
	.section .gcc_except_table,"a",%progbits
	.p2align 4
.LLSDA:
	.4byte 0x11223344
	.4byte 0x99aabbcc
	.globl xlsda
xlsda:
	.4byte 0x11223344
	.4byte DW.ref.pp-.
	.4byte 0x55667788
	.4byte 0
