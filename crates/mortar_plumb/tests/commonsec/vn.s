# WP-210: an input section named COMMON, NOBITS (lld's -r of commons, A120).
	.section COMMON,"aw",@nobits
	.balign 8
	.globl cx
cx:
	.zero 0x1000
