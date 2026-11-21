# WP-210: COMMON.x, an orphan GNU ld places after .bss.
	.section COMMON.x,"aw",@nobits
	.balign 8
	.globl cxx
cxx:
	.zero 16
