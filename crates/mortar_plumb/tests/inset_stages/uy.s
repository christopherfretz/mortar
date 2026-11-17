# libuy.so: defines y, refers to g.
	.globl y
	.type y, %object
	.data
y:
	.dc.a g
	.size y, . - y
