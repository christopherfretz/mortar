#include <stdio.h>
static int impl1(void){return 41;}
static int impl2(void){return 42;}
static int counter;
int *volatile pc = &counter;
/* impl2 only if pc's RELATIVE record was applied before this resolver ran */
static int (*resolve(void))(void){ return pc == &counter ? impl2 : impl1; }
int f(void) __attribute__((ifunc("resolve")));
int main(void){ printf("%d\n", f()); return f() == 42 ? 0 : 1; }
