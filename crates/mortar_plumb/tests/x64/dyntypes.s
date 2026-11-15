# W3: dynamic-only types in an input; GNU ld writes S + A (spec/x64.md W3, X-14).
	.text
	.globl _start
_start: ret
x: ret
	.data
	.reloc ., R_X86_64_DTPMOD64, x
	.quad 0
	.reloc ., R_X86_64_IRELATIVE, x
	.quad 0
	.reloc ., R_X86_64_RELATIVE, x
	.quad 0
