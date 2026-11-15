# The i386 rewriting forms (spec/plumb.md §5.11; spec/x86.md §8, §9):
# GOT32X G1-G5 with a base register, GOT32 kept, TLS I1-I5, D1, D2, L1,
# L2, T1 and T2, against local, global, absolute, undefined-weak and
# thread-local targets. The no-base GOT forms (mold's W3) and D3 (mold's
# W4) are cases of their own.
	.text
	.globl _start
_start:
	call 1f
1:	popl %ebx
	addl $_GLOBAL_OFFSET_TABLE_+(.-1b), %ebx
	movl $1, %eax
	xorl %ebx, %ebx
	int $0x80
# ---- GOT32X, base register ----
g_mov:
	movl foo@GOT(%ebx), %eax
	movl lfoo@GOT(%ebx), %ecx
	movl wund@GOT(%ebx), %edx
	movl absv@GOT(%ebx), %esi
	movl foo@GOT(%ecx), %edi
g_binop:
	addl foo@GOT(%ebx), %eax
	orl  foo@GOT(%ebx), %ecx
	adcl foo@GOT(%ebx), %edx
	sbbl foo@GOT(%ebx), %esi
	andl foo@GOT(%ebx), %edi
	subl foo@GOT(%ebx), %ebp
	xorl foo@GOT(%ebx), %eax
	cmpl foo@GOT(%ebx), %ecx
	testl %eax, foo@GOT(%ebx)
	testl %edx, lfoo@GOT(%ebx)
g_call:
	call *fn@GOT(%ebx)
	jmp *fn@GOT(%ebx)
	call *wundf@GOT(%ebx)
	jmp *wundf@GOT(%ebx)
# ---- GOT32 (not relaxable) ----
g_got32:
	.byte 0x8b, 0x83
	.reloc ., R_386_GOT32, foo
	.long 0
	pushl foo@GOT(%ebx)
# ---- TLS initial-exec ----
ie_all:
	movl ie@indntpoff, %eax
	movl ie@indntpoff, %ecx
	addl ie@indntpoff, %edx
	movl ie@gotntpoff(%ebx), %esi
	addl ie@gotntpoff(%ebx), %edi
	movl wtls@gotntpoff(%ebx), %eax
	movl %gs:0, %ecx
	movl %gs:(%ecx), %eax
# ---- TLS general-dynamic ----
gd_d1:
	leal gd@tlsgd(,%ebx,1), %eax
	call ___tls_get_addr@PLT
gd_d2:
	leal gd@tlsgd(%ebx), %eax
	call *___tls_get_addr@GOT(%ebx)
# ---- TLS local-dynamic ----
ld_l1:
	leal ld@tlsldm(%ebx), %eax
	call ___tls_get_addr@PLT
	leal ld@dtpoff(%eax), %edx
	movl ld@dtpoff(%eax), %ecx
ld_l2:
	leal ld@tlsldm(%ebx), %eax
	call *___tls_get_addr@GOT(%ebx)
	leal lev@dtpoff(%eax), %edx
# ---- TLS descriptors ----
desc:
	leal gd@tlsdesc(%ebx), %eax
	call *gd@tlscall(%eax)
	leal ie2@tlsdesc(%ebx), %eax
	call *ie2@tlscall(%eax)
# ---- TLS local-exec ----
le_code:
	movl %gs:lev@ntpoff, %eax
	movl %gs:0, %eax
	leal lev@ntpoff(%eax), %eax
	.long lev@ntpoff
end_text:
	nop
fn:	ret
	.globl ___tls_get_addr
___tls_get_addr: ret
	.data
	.globl foo
foo:	.long 0
lfoo:	.long 0
	.weak wund
	.weak wundf
	.weak wtls
	.set absv, 0x1234
	.globl absv
	.section .tdata,"awT",@progbits
	.globl gd
gd:	.long 1
	.globl ie
ie:	.long 2
ie2:	.long 3
	.section .tbss,"awT",@nobits
ld:	.zero 4
lev:	.zero 4
	.type wtls, @tls_object

	.section .debug_info,"",@progbits
	.long ld@dtpoff
	.long gd@dtpoff
