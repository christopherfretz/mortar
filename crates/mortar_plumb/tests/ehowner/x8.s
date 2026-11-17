// x.s with fx8's FDE (and its CIE) written by hand in the 64-bit DWARF
// form (length 0xffffffff, then an 8-byte length; 8-byte ids): the live
// FDE reads its LSDA at xlsda, a global of tb.o's table.
	.text
	.globl fx8
	.type fx8, %function
fx8:
	ret
	.size fx8, .-fx8
	.section .eh_frame,"a",%progbits
	.p2align 3
.Lcie:
	.4byte 0xffffffff
	.8byte .Lcie_end-.Lcie_id
.Lcie_id:
	.8byte 0
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
.Lfde:
	.4byte 0xffffffff
	.8byte .Lfde_end-.Lfde_id
.Lfde_id:
	.8byte .Lfde_id-.Lcie
	.4byte fx8-.
	.4byte 4
	.uleb128 4
	.4byte xlsda-.
	.p2align 3, 0
.Lfde_end:
