# An empty SHF_MERGE section: the global `gms`, the local `lms`; an empty
# .rodata.cst8 with the weak `wcs` (synth.md S-6: placed whole).
	.section .rodata.str1.1,"aMS",%progbits,1
	.globl	gms
gms:
lms:
	.section .rodata.cst8,"aM",%progbits,8
	.weak	wcs
wcs:
	.section .note.GNU-stack,"",%progbits
