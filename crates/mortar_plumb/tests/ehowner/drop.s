// g's FDE is in an .eh_frame that is a member of g's group, whose copy
// here is dropped: the FDE is not in any .eh_frame the link reads. Its
// LSDA starts at the table's offset 0; the word after names the local
// DW.ref.pp of the dropped copy: no FDE the link reads owns it.
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
	ret
	.size g, .-g
	.section .eh_frame,"aG",%progbits,g,comdat
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
	.4byte g-.
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
