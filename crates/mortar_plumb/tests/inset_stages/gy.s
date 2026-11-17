# libgy.a(gy.o): defines g and y.
	.globl g, y
	.type g, %object
	.type y, %object
	.data
g:
	.dc.a 1
	.size g, . - g
y:
	.dc.a 2
	.size y, . - y
