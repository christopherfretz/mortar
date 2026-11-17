// An FDE whose initial location has no relocation and is PC-relative
// (0x100, encoding 0x1b): §6.4 cannot resolve it, so its life is
// unknown, and the word of the LSDA it reads has no owner the verifier
// can name.
	.section .data.rel.local.DW.ref.pp,"awG",%progbits,DW.ref.pp,comdat
	.p2align 3
	.type DW.ref.pp, %object
	.size DW.ref.pp, 8
DW.ref.pp:
	.8byte 0
	.section .eh_frame,"a",%progbits
	.p2align 3
.Lcie:
	.4byte .Lcie_end-.Lcie_id
.Lcie_id:
	.4byte 0
	.byte 1
	.asciz "zLR"
	.uleb128 4
	.sleb128 -8
	.byte 30
	.uleb128 2
	.byte 0x1b
	.byte 0x1b
	.byte 0x0c, 31, 0
	.p2align 3, 0
.Lcie_end:
	.4byte .Lfde_end-.Lfde_id
.Lfde_id:
	.4byte .Lfde_id-.Lcie
	.4byte 0x100
	.4byte 4
	.uleb128 4
	.4byte .LLSDA-.
	.p2align 3, 0
.Lfde_end:
	.section .gcc_except_table,"a",%progbits
	.p2align 4
.LLSDA:
	.4byte 0x11223344
	.4byte DW.ref.pp-.
	.4byte 0x55667788
	.4byte 0
