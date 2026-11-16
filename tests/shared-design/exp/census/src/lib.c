/* census library, C */
#include <stdio.h>
int lib_data = 41;
int lib_inited = 0;
static int helper(int x) { return x + 1; }
int lib_func(void) { return helper(lib_data); }
void my_init(void) { lib_inited = 1; }
void my_fini(void) { }
int lib_check(void) { printf("lib C data=%d\n", lib_data); return lib_func(); }
