/* An IFUNC taken as a data word and as a PC-relative address: fp == l is
   A32 on x86-64 GNU ld and mold (prints 0), 1 elsewhere. */
#include <stdio.h>
static int impl1(void){return 41;}
static int impl2(void){return 42;}
static int counter;
int *volatile pc = &counter;
static int (*resolve(void))(void){ return pc ? impl2 : impl1; }
int f(void) __attribute__((ifunc("resolve")));
int (*fp)(void) = f;
int (*const cfp)(void) = f;
int main(void){ int (*volatile l)(void) = f; printf("%d %d %d %d\n", f(), fp(), cfp(), fp == l); return f() == 42 ? 0 : 1; }
