#include "asm.h"
// 2 MiB of `.text` after f: val more than 1 MiB past f's ADRP.
 .text
 .globl gap
gap:
 ret
 .skip 0x200000
