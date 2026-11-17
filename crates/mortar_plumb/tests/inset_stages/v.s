# libv.so: defines v.
	.globl v
	.type v, %object
	.data
v:
	.dc.a 0
	.size v, . - v
