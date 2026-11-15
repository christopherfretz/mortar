# spec/x86.md R-13 (b): two records at one r_offset, and a field that
# overlaps another record's field.
	.text
	.globl	_start
_start:
	ret
	.data
d:
	.reloc	d, R_386_32, _start
	.reloc	d, R_386_32, _start
	.long	0
	.reloc	d+4, R_386_32, _start
	.reloc	d+6, R_386_32, _start
	.long	0
	.long	0
