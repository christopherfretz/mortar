// WP-210: references the data-end names and the COMMON bounds (weak, so
// every linker links them) and an input COMMON's `cx` from .data, which
// _start keeps live.
	.text
	.globl _start
_start:
	adrp x0, refs
	add x0, x0, :lo12:refs
	b .
	.data
refs:
	.quad cx
	.quad _edata, edata, __bss_start, __bss_start__, __bss_end__, _bss_end__
	.quad _end, __start_COMMON, __stop_COMMON
	.weak cx, _edata, edata, __bss_start, __bss_start__, __bss_end__, _bss_end__
	.weak _end, __start_COMMON, __stop_COMMON
	.bss
	.zero 16
