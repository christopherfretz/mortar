	.text
	.globl _start
_start:
	.dc.a empty_live
	.section .text.empty_live,"ax",%progbits
	.globl empty_live
empty_live:
	.section .text.empty_dead,"ax",%progbits
	.globl empty_dead
empty_dead:
	.section .text.full_dead,"ax",%progbits
	.globl full_dead
full_dead:
	.byte 0, 0, 0, 0
	.section .debug_addr,"",%progbits
	.dc.a empty_dead, empty_live, full_dead
	.section .debug_rnglists,"",%progbits
	.dc.a empty_dead, empty_live, full_dead
	.section .debug_ranges,"",%progbits
	.dc.a empty_dead, empty_live, full_dead
	.section .debug_loc,"",%progbits
	.dc.a empty_dead, empty_live, full_dead
	.section .debug_info,"",%progbits
	.dc.a empty_dead, empty_live, full_dead
