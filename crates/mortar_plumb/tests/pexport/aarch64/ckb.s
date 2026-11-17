// WP-147b: one weak COMDAT copy of an inline function (_Z4pickv) and of
// an inline constructor (_ZN3BoxC1Ev and C2, one group) per file; ckb.s
// alone has a .rodata, so a map names it first (tests/pexport.rs).
	.section	.text._Z4pickv,"axG",@progbits,_Z4pickv,comdat
	.weak	_Z4pickv
	.type	_Z4pickv, %function
_Z4pickv:
	mov	w0, #40
	ret
	.size	_Z4pickv, .-_Z4pickv
	.section	.text._ZN3BoxC2Ev,"axG",@progbits,_ZN3BoxC5Ev,comdat
	.weak	_ZN3BoxC2Ev
	.type	_ZN3BoxC2Ev, %function
_ZN3BoxC2Ev:
	mov	w1, #7
	str	w1, [x0]
	ret
	.size	_ZN3BoxC2Ev, .-_ZN3BoxC2Ev
	.weak	_ZN3BoxC1Ev
	.set	_ZN3BoxC1Ev,_ZN3BoxC2Ev
	.text
	.globl	kb
	.type	kb, %function
kb:
	ret
	.size	kb, .-kb
	.section	.rodata
	.globl	kbtab
	.type	kbtab, %object
kbtab:
	.word	1, 2, 3, 4
	.size	kbtab, 16
	.section	.note.GNU-stack,"",@progbits
