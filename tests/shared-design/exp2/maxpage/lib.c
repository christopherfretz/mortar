#include <stdio.h>
const char *const tab[] = {"relro-a", "relro-b"};   /* .data.rel.ro: RELRO */
int counter = 5;                                     /* .data: after RELRO */
static int (*fp)(int) = 0;
static int twice(int x) { return 2 * x; }
__attribute__((constructor)) static void init(void) { fp = twice; }
int f(int i) { counter++; return fp(i) + (int)tab[i & 1][6] + counter; }
