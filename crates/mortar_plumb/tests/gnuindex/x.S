/* WP-251: .debug_gnu_pubnames in odd places, as WP-229's lane test:
   GROUP, in COMDAT group xgrp with an allocated member; ALLOC, an
   allocated section of that name (not debug information). */
	.text
	.globl xfn
	.type xfn, %function
xfn:
	ret
	.size xfn, .-xfn
#ifdef GROUP
	.section .text.xg,"axG",%progbits,xgrp,comdat
	.globl xg
	.type xg, %function
xg:
	ret
	.size xg, .-xg
	.section .debug_gnu_pubnames,"G",%progbits,xgrp,comdat
#else
	.section .debug_gnu_pubnames,"a",%progbits
#endif
	.long 14
	.short 2
	.long 0
	.long 0
	.long 0
	.byte 0, 0, 0, 0
