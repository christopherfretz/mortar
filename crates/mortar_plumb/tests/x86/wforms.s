# spec/x86.md §15.2's wrong-code inputs, one per --defsym:
#   W1: `movl foo@GOT(%ebp,%ecx,1)`, SIB 0d after the ModRM: two readings
#       (X86-13); GNU ld rewrites it into garbage, lld writes G + A.
#   W2: the moffs store `movl %eax, foo@GOT` (a3): the base reading.
#   W3: `call *foo@GOT` with no base (GCC -fno-pic -fno-plt): mold
#       writes G − .got.
#   W4: GD `lea x@tlsgd(%ebx)`, direct call, nop (D3): mold writes its
#       LE form from P − 3.
	.text
	.globl _start
_start:
	call 1f
1:	popl %ebx
	addl $_GLOBAL_OFFSET_TABLE_+(.-1b), %ebx
.ifdef W1
	movl %ebx, %ebp
	xorl %ecx, %ecx
	movl foo@GOT(%ebp,%ecx,1), %eax
.endif
.ifdef W2
	movl %eax, foo@GOT
.endif
.ifdef W3
	call *fn@GOT
	movl foo@GOT, %eax
.endif
.ifdef W4
	nop
	leal gd@tlsgd(%ebx), %eax
	call ___tls_get_addr@PLT
	nop
.endif
	movl $1, %eax
	xorl %ebx, %ebx
	int $0x80
fn:	ret
	.globl ___tls_get_addr
___tls_get_addr: ret
	.data
	.globl foo
foo:	.long 0
	.section .tdata,"awT",@progbits
	.globl gd
gd:	.long 1
