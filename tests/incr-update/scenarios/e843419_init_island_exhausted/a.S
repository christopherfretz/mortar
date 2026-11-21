#include "asm.h"
// _start calls f and prints what it returns (42: `j`). No executable
// section follows `.text` (g.o's 2 MiB is `.text` too, after c.o): the
// island after `.text` is the last chunk of the RX PT_LOAD, with no
// capacity (L-9).
 start
 bl f
 print_exit
 .data
 .balign 8
 .globl val
val:
 .xword 42
