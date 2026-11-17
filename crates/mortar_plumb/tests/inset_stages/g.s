# libg.a(g.o): defines g, with a constructor (an extraction changes the
# program).
	.globl g
	.type g, %object
	.data
g:
	.dc.a 1
	.size g, . - g
	.section .init_array, "aw"
	.dc.a g
