// WP-210's hand-made .eh_frame for tests/ehlocal.rs: one CIE, the FDE of
// the live _start and the FDE of the dead function, with local labels at
// S-50's offsets.
	.section .text._start,"ax",%progbits
	.globl _start
	.type _start,%function
_start:
	mov x0, #0
	b .
	.section .text.dead,"ax",%progbits
	.type dead,%function
dead:
	ret
	.section .eh_frame,"a",%progbits
	.balign 8
eh_begin:			// r = 0: the output's start (rule 1)
cie:
	.long cie_end - cie_id
cie_id:
	.long 0
	.byte 1
	.asciz "zR"
	.uleb128 1
	.sleb128 -8
	.uleb128 30
	.uleb128 1
	.byte 0x1b
	.balign 4, 0
cie_end:
fde_live:
	.long fl_end - fl_cie
fl_cie:
	.long fl_cie - cie
fl_mid:				// inside a kept record: its place (rule 2)
	.long _start - .
	.long 8
	.uleb128 0
	.balign 4, 0
fl_end:
fde_dead:			// at a removed record: the next kept one's place (rule 3)
	.long fd_end - fd_cie
fd_cie:
	.long fd_cie - cie
fd_mid:				// inside a removed record, no relocation names it: dropped (rule 4)
	.long dead - .
	.long 4
	.uleb128 0
	.balign 4, 0
fd_end:
eh_end:				// r = sh_size: rule 3
