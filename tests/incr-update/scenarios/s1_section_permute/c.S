#include "asm.h"
 start
 callf f
#ifndef C
 addv 10
#else
 addv 20
#endif
 print_exit
