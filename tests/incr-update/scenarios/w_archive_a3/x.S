#include "asm.h"
 start
 callf g
 callf f
#ifndef EDIT
 addv 10
#else
 addv 20
#endif
 print_exit
