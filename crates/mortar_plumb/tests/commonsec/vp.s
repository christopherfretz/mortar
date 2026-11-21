# WP-210: an input section named COMMON, PROGBITS (.bss becomes PROGBITS).
	.section COMMON,"aw",@progbits
	.balign 8
	.globl cp
cp:
	.quad 1
