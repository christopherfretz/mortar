# W6: every input IBT|SHSTK; mold marks the output IBT and writes IPLT
# entries without endbr64 (spec/x64.md X-38, W6).
	.text
	.globl _start
_start:
	call ifn@PLT
	call ifn2
	movq $ifn, %rax
	leaq ifn(%rip), %rbx
	movq ifn@GOTPCREL(%rip), %rcx
	leaq _GLOBAL_OFFSET_TABLE_(%rip), %rdx
	movq $__rela_iplt_start, %rsi
	movq $__rela_iplt_end, %rdi
	mov $60,%eax
	syscall
	.type ifn, @gnu_indirect_function
	.globl ifn
ifn:	lea fn(%rip), %rax
	ret
	.type ifn2, @gnu_indirect_function
ifn2:	lea fn(%rip), %rax
	ret
fn:	ret
	.data
	.quad ifn
	.section .note.gnu.property,"a"
	.p2align 3
	.long 4, 16, 5
	.asciz "GNU"
	.long 0xc0000002, 4, 3, 0
