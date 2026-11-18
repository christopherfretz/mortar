#include "asm.h"
 .section .rodata.str1.1,"aMS",@progbits,1
Labc: .asciz "abc"
 .section .rodata.str1.8,"aMS",@progbits,1
 .balign 8
Lxyz: .asciz "xyz"
 start
 write_str Labc, 3
 write_str Lxyz, 3
 retv 0
 print_exit
