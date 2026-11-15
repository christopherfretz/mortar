# X-48: a call to __tls_get_addr with no TLSGD/TLSLD before it, which no
# relaxation consumes; it survives as a call in a static output.
	.text
	.globl _start
	.type _start,@function
_start:
	call __tls_get_addr@PLT
	movq __tls_get_addr@GOTPCREL(%rip), %rax
	ret
	.globl __tls_get_addr
	.type __tls_get_addr,@function
__tls_get_addr:
	ret
	.section .note.GNU-stack,"",@progbits
