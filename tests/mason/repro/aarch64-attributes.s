	.aeabi_subsection aeabi_feature_and_bits, optional, uleb128
	.aeabi_attribute Tag_Feature_BTI, 1
	.aeabi_attribute Tag_Feature_PAC, 1
	.aeabi_attribute Tag_Feature_GCS, 0
	.text
	.globl _start
_start:
	mov x0, #0
	mov x8, #93
	svc #0
