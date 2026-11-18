/* WP-250: a header every unit includes. GCC -g3 puts the macros of each
   header in a .debug_macro COMDAT group (wm4.<file>.<line>.<md5>) and each
   unit imports it by offset (DW_MACRO_import); the copies of b.o and c.o
   are dropped. No library is linked (-nostdlib): never run. */
#ifndef WP250_H
#define WP250_H
#include <stddef.h>
#include <stdint.h>
#define WP250_ANSWER 42
#define WP250_TWICE(x) ((x) * 2)
struct wp250_point { int x; long y; };
int b_f(int);
int c_f(int);
extern int g_b;
#endif
