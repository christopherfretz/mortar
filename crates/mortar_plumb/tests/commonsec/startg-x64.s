# WP-210 (gc): start-x64.s without the COMMON bounds (GNU ld keeps a
# section X whose __start_X is referenced): the data-end names (weak,
# so every linker links them) and an input COMMON's `cx` from .data,
# which _start keeps live.
	.text
	.globl _start
_start:
	lea refs(%rip), %rax
	jmp .
	.data
refs:
	.quad cx
	.quad _edata, edata, __bss_start, __bss_start__, __bss_end__, _bss_end__
	.quad _end
	.weak cx, _edata, edata, __bss_start, __bss_start__, __bss_end__, _bss_end__
	.weak _end
	.bss
	.zero 16
