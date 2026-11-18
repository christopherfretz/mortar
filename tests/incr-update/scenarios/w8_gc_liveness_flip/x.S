#include "asm.h"
 start
#ifndef EDIT
 callf f
#else
 callf g
#endif
 addv 10
 print_exit
