#include "asm.h"
 .section .rodata.str1.1,"aMS",@progbits,1
Lfoo: .asciz "foo"
 start
 write_str Lfoo, 3
#ifndef C
 retv 10
#else
 retv 20
#endif
 print_exit
