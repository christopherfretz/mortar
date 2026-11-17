	.section .text.cd,"axG",%progbits,cd_fn,comdat
	.weak cd_fn
cd_fn:
.Lcd:
	.section .debug_addr,"",%progbits
	.dc.a .Lcd
	.section .debug_rnglists,"",%progbits
	.dc.a .Lcd
	.section .debug_ranges,"",%progbits
	.dc.a .Lcd
	.section .debug_loc,"",%progbits
	.dc.a .Lcd
	.section .debug_info,"",%progbits
	.dc.a .Lcd
