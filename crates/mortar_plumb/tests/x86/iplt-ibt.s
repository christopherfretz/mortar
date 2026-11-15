# X86-49: every input IBT|SHSTK; the IPLT entries of the IBT output start
# with endbr32 (GNU ld and mold; lld 22 does not mark an i386 output).
	.text
	.globl _start
_start:
	endbr32
	call ifn@PLT
	call ifn2
	movl $ifn, %eax
	movl $__rel_iplt_start, %esi
	movl $__rel_iplt_end, %edi
	movl $1, %eax
	xorl %ebx, %ebx
	int $0x80
	.type ifn, @gnu_indirect_function
	.globl ifn
ifn:	endbr32
	movl $fn, %eax
	ret
	.type ifn2, @gnu_indirect_function
ifn2:	endbr32
	movl $fn, %eax
	ret
fn:	endbr32
	ret
	.data
	.long ifn
	.section .note.gnu.property,"a"
	.p2align 2
	.long 4, 12, 5
	.asciz "GNU"
	.long 0xc0000002, 4, 3
