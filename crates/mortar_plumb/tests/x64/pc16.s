# W2: GNU ld writes PC16 = +32768 and -32769 (spec/x64.md W2); a violation (R2).
	.text
	.globl _start
_start: ret
	.data
	.reloc ., R_X86_64_PC16, far16
	.short 0
	.reloc ., R_X86_64_PC8, far16b
	.byte 0
