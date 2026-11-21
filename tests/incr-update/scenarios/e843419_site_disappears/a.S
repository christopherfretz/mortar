#include "asm.h"
// _start calls f and prints what it returns (42: `j`). `.after`, an
// executable output section after `.text` (R-4), gives `.text`'s island
// capacity (L-9: the last chunk of a PT_LOAD has none) and puts `val`
// more than 1 MiB past f (the ADR fix out of reach: a site takes a patch).
 start
 bl f
 print_exit
 .section .after,"ax",@progbits
 .globl after
 .type after,%function
after:
 ret
 .skip 0x200000
 .data
 .balign 8
 .globl val
val:
 .xword 42
