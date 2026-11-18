#include "asm.h"
 .section .rodata.str1.1,"aMS",@progbits,1
Lf: .asciz "foo"
 start
 write_str Lf, 3
 retv 12
 print_exit
