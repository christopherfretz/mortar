#include "asm.h"
// EDIT: another value added after the call, at the same size.
 start
 callf site
#ifndef EDIT
 addv 1
#else
 addv 3
#endif
 print_exit
