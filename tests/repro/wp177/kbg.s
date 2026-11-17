	.section .data.rel.local.DW.ref.pp,"awG",@progbits,DW.ref.pp,comdat
	.p2align 3
	.weak DW.ref.pp
	.hidden DW.ref.pp
	.type DW.ref.pp, %object
	.size DW.ref.pp, 8
DW.ref.pp:
	.xword 0x1234
	.section .text.F,"ax",@progbits
	.globl F
	.type F,%function
F:
	.cfi_startproc
	.cfi_personality 0x9b, DW.ref.pp
	.cfi_lsda 0x1b, .LLSDA
	ret
	.cfi_endproc
	.section .gcc_except_table,"a",@progbits
.LLSDA:
	.byte 0xff, 0x9b
	.uleb128 .Lend-.Lstart
.Lstart:
	.byte 0x1
	.uleb128 0
	.p2align 2
	.4byte DW.ref.pp-.
.Lend:
