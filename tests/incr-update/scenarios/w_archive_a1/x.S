#include "asm.h"
 start
 callf g
#ifndef EDIT
 callf f
#else
 callf h
#endif
 addv 10
 print_exit
