/* c3.c: a .ctors entry with a priority (.ctors.65335 is priority 200). */
#include "rec.h"
static void ctor_ctors(void) { rec(3); }
__attribute__((section(".ctors.65335"), used)) static void (*ctors_p)(void) = ctor_ctors;
