# WP-210: an unreferenced input COMMON that --gc-sections removes.
	.section COMMON,"aw",@nobits
	.balign 8
	.globl gx
gx:
	.zero 32
