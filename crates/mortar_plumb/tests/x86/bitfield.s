# W11 (spec/x86.md §15.2): R_386_16 and R_386_8 of m1 and m2, given by
# --defsym; -32769 and -129 are out of range, which GNU ld writes truncated.
	.text
	.globl _start
_start: ret
	.data
	.reloc ., R_386_16, m1
	.short 0
	.reloc ., R_386_8, m2
	.byte 0
