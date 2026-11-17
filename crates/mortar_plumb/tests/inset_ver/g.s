# libg.a(g.o): the plain g, as libgcc.a(_udivdi3.o)'s __udivti3; and a
# constructor, so an extraction would change the program.
	.globl g
	.type g, %object
	.data
g:
	.dc.a 1
	.size g, . - g
	.section .init_array, "aw"
	.dc.a g
