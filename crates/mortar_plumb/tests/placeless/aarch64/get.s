	.text
	.globl get
	.type get,%function
get:
	mov x0, #0
	mov x8, #93
	svc #0
