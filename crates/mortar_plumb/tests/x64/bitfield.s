# W1 (spec/x64.md §14.2): R_X86_64_16 and _8 of m1 and m2, given by
# --defsym; -32769 and -129 are out of range, which GNU ld writes truncated.
	.text
	.globl _start
_start: ret
	.data
	.reloc ., R_X86_64_16, m1
	.short 0
	.reloc ., R_X86_64_8, m2
	.byte 0
