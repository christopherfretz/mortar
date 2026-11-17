# libu.so: defines u, refers to g.
	.globl u
	.type u, %object
	.data
u:
	.dc.a g
	.size u, . - u
