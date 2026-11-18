#include "asm.h"
// A: the same two functions, their sections in the other order.
#ifndef A
 fn f, globl, 1
 fn g, globl, 2
#else
 fn g, globl, 2
 fn f, globl, 1
#endif
