/* c1.c: first object of the C component (WP-154 corpus). */
#include "rec.h"
static int counter;                      /* local */
int g_data = 5;                          /* global data */
int common_x;                            /* common (-fcommon) */
const char *msg1(void) { return "hello, component"; }   /* .rodata.str1.* */
__attribute__((visibility("hidden"))) int hid(void) { return 7; }
__attribute__((weak)) int wk(void) { return 1; }        /* overridden by c2 */
__attribute__((weak)) int wk_only(void) { return 11; }  /* weak, alone */
extern int undef_weak(void) __attribute__((weak));
int bump(void) { return ++counter; }
int call_undef_weak(void) { return undef_weak ? undef_weak() : -1; }
__attribute__((constructor(200))) static void ctor200(void) { rec(200); }
__attribute__((constructor)) static void ctor_default1(void) { rec(1); }
