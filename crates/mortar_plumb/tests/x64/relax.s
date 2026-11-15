# The x86-64 rewriting forms (spec/plumb.md §5.9; spec/x64.md §8): every
# GOTPCRELX, GOTTPOFF, TLSGD, TLSLD and TLSDESC form, against local,
# absolute, undefined-weak and IFUNC targets. GDL/LDL add the large-model
# sequences (lld refuses LDL, X-27; its GDL is W4).
	.text
	.globl _start
_start:
	mov $60,%eax
	xor %edi,%edi
	syscall
# ---- GOTPCRELX family ----
	.globl g_mov
g_mov:
	movq foo@GOTPCREL(%rip), %rax
	movq foo@GOTPCREL(%rip), %r9
	movl foo@GOTPCREL(%rip), %eax
	movq lfoo@GOTPCREL(%rip), %rcx
	movq wund@GOTPCREL(%rip), %rdx
	movq ifn@GOTPCREL(%rip), %rsi
	movq abs1@GOTPCREL(%rip), %rdi
	movq absbig@GOTPCREL(%rip), %r10
	movq fn@GOTPCREL(%rip), %r11
g_call:
	call *fn@GOTPCREL(%rip)
	jmp *fn@GOTPCREL(%rip)
	call *wundf@GOTPCREL(%rip)
	jmp *wundf@GOTPCREL(%rip)
	call *ifn@GOTPCREL(%rip)
	call *foo@GOTPCREL(%rip)
g_binop:
	testq %rax, foo@GOTPCREL(%rip)
	testq %r12, foo@GOTPCREL(%rip)
	addq foo@GOTPCREL(%rip), %rax
	adcq foo@GOTPCREL(%rip), %r13
	andq foo@GOTPCREL(%rip), %rbx
	cmpq foo@GOTPCREL(%rip), %rcx
	orq  foo@GOTPCREL(%rip), %rdx
	sbbq foo@GOTPCREL(%rip), %rsi
	subq foo@GOTPCREL(%rip), %rdi
	xorq foo@GOTPCREL(%rip), %r8
	testl %eax, foo@GOTPCREL(%rip)
	addl foo@GOTPCREL(%rip), %eax
	cmpq wund@GOTPCREL(%rip), %rax
g_plain:
	.reloc .+3, R_X86_64_GOTPCREL, foo-4
	movq 0(%rip), %rax
	cmpq $0, foo@GOTPCREL(%rip)
	pushq foo@GOTPCREL(%rip)
	movq foo@GOTPCREL(%rip), %xmm0
g_apx:
# ---- GOTTPOFF ----
ie_all:
	movq ie@gottpoff(%rip), %rax
	movq ie@gottpoff(%rip), %rsp
	movq ie@gottpoff(%rip), %r12
	movq ie@gottpoff(%rip), %r15
	addq ie@gottpoff(%rip), %rax
	addq ie@gottpoff(%rip), %rsp
	addq ie@gottpoff(%rip), %r12
	addq ie@gottpoff(%rip), %r13
	movq wtls@gottpoff(%rip), %rax
# ---- TLSGD ----
gd16:
	.byte 0x66
	leaq gd@tlsgd(%rip), %rdi
	.byte 0x66,0x66,0x48
	call __tls_get_addr@PLT
gd16w:
	.byte 0x66
	leaq wtls@tlsgd(%rip), %rdi
	.byte 0x66,0x66,0x48
	call __tls_get_addr@PLT
gdnoplt:
	.byte 0x66
	leaq gd@tlsgd(%rip), %rdi
	.byte 0x66,0x48
	call *__tls_get_addr@GOTPCREL(%rip)
.ifdef GDL
gdlarge:
	leaq gd@tlsgd(%rip), %rdi
	movabsq $__tls_get_addr@PLTOFF, %rax
	addq %r15, %rax
	call *%rax
.endif
# ---- TLSLD ----
ld12:
	leaq ld@tlsld(%rip), %rdi
	call __tls_get_addr@PLT
	leaq ld@dtpoff(%rax), %rcx
	movl ld@dtpoff(%rax), %edx
ldnoplt:
	leaq ld@tlsld(%rip), %rdi
	call *__tls_get_addr@GOTPCREL(%rip)
.ifdef LDL
ldlarge:
	leaq ld@tlsld(%rip), %rdi
	movabsq $__tls_get_addr@PLTOFF, %rax
	addq %r15, %rax
	call *%rax
.endif
# ---- TLSDESC ----
desc_rax:
	leaq gd@tlsdesc(%rip), %rax
	call *gd@tlscall(%rax)
desc_rbp:
	leaq ie2@tlsdesc(%rip), %rbp
	movq %rbp, %rax
	call *ie2@tlscall(%rax)
	movq %rbp, %rax
	call *ie2@tlscall(%rax)
desc_r12:
	leaq gd@tlsdesc(%rip), %r12
	movq %r12, %rax
	call *gd@tlscall(%rax)
desc_w:
	leaq wtls@tlsdesc(%rip), %rax
	call *wtls@tlscall(%rax)
desc_base:
	leaq _TLS_MODULE_BASE_@tlsdesc(%rip), %rax
	call *_TLS_MODULE_BASE_@tlscall(%rax)
	leaq ld@dtpoff(%rax), %rcx
le_code:
	movq %fs:0, %rax
	leaq lev@tpoff(%rax), %rax
	movl %fs:lev@tpoff, %eax
	.long lev@tpoff
end_text:
	nop
	.type ifn, @gnu_indirect_function
ifn:	lea fn(%rip), %rax
	ret
fn:	ret
	.globl __tls_get_addr
__tls_get_addr: ret
	.data
	.globl foo
foo:	.quad 0
lfoo:	.quad 0
	.weak wund
	.weak wundf
	.weak wtls
	.set abs1, 0x1234
	.globl abs1
	.set absbig, 0x123456789
	.globl absbig
	.section .tdata,"awT",@progbits
	.globl gd
gd:	.quad 1
	.globl ie
ie:	.quad 2
ie2:	.quad 3
	.section .tbss,"awT",@nobits
ld:	.zero 8
lev:	.zero 8
	.type wtls, @tls_object

	.section .debug_info,"",@progbits
	.quad ld@dtpoff
	.quad gd@dtpoff
