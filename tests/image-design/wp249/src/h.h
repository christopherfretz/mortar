/* WP-249: a header every unit includes: macros (GCC -g3 puts them in a
   .debug_macro COMDAT group), a struct (a type unit under
   -fdebug-types-section). */
#ifndef WP249_H
#define WP249_H
#define WP249_ANSWER 42
#define WP249_TWICE(x) ((x) * 2)
struct wp249_point { int x; long y; };
int b_f(int);
int c_f(int);
extern int g_b;
#endif
