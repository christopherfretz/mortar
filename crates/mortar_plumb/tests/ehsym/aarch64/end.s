# A terminator with a global at its end (offset 4 = sh_size): synth.md
# S-50 rule 3 (S-48a when no record is live).
	.section .eh_frame,"a",%progbits
	.long	0
	.globl	frame_end_after
	.type	frame_end_after, %object
frame_end_after:
