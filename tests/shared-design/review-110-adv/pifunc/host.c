#include <stdio.h>
int f(void); void *lib_addr(void); extern void *lib_data; int lib_call(void);
int main(void){ void *e=(void*)f; int ok = e==lib_addr() && e==lib_data && f()==42 && lib_call()==42;
 printf("%s exe&f=%p lib&f=%p libdata=%p\n", ok?"OK":"FAIL", e, lib_addr(), lib_data); return !ok; }
