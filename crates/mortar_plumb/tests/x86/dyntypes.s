# W14: dynamic-only types in an input; GNU ld writes S + A (spec/x86.md
# W14, X86-20).
	.text
	.globl _start
_start: ret
x: ret
	.data
	.reloc ., R_386_GLOB_DAT, x
	.long 0
	.reloc ., R_386_IRELATIVE, x
	.long 0
	.reloc ., R_386_RELATIVE, x
	.long 0x10
