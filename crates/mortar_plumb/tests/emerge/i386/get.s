	.text
	.globl get
	.type get,@function
get:
	mov $1,%eax; xor %ebx,%ebx; int $0x80
