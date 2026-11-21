#include "asm.h"
// val, the site's target V: the edit puts 24 bytes before it (inside
// d.o's region slack), so W4' (`ldr x0, [x0, #:lo12:val]`) changes.
 .data
 .balign 8
#ifdef A
 .xword 1, 2, 3
#endif
 .globl val
val:
 .xword 42
