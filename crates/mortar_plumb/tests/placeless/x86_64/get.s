	.text
	.globl get
	.type get,@function
get:
	mov $60,%eax; xor %edi,%edi; syscall
