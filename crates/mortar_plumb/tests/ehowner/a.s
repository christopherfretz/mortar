// The kept copies (first on the line) of the groups DW.ref.pp and g.
	.section .data.rel.local.DW.ref.pp,"awG",%progbits,DW.ref.pp,comdat
	.p2align 3
	.weak DW.ref.pp
	.hidden DW.ref.pp
	.type DW.ref.pp, %object
	.size DW.ref.pp, 8
DW.ref.pp:
	.8byte 0
	.section .text.g,"axG",%progbits,g,comdat
	.weak g
	.type g, %function
g:
	ret
