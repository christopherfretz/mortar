/* A30: the GOT slot of an absolute symbol (value 2) must hold 2 at any load bias. */
#include <stdio.h>
extern char absv[];
__asm__(".globl absv\n.set absv, 2\n");
char *__attribute__((noinline)) q(void) { return absv; }
int main(void) { char *v = q(); printf("q=%p\n", (void *)v); return v != (char *)2; }
