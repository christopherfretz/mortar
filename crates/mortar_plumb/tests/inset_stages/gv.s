# libgv.a(gv.o): defines g, refers to v (libv.so joins N at stage 2).
	.globl g
	.type g, %object
	.data
g:
	.dc.a v
	.size g, . - g
