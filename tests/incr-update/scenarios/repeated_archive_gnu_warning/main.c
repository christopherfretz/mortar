/* The first caller. CALLER: it calls the warned foo too. */
#include <stdio.h>
int cfun(void);
#ifdef CALLER
int foo(int);
int main(void) { printf("%d\n", cfun() + foo(10)); return 0; }
#else
int main(void) { printf("%d\n", cfun()); return 0; }
#endif
