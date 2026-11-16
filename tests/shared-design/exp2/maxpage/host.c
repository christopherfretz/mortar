#include <stdio.h>
int f(int);
int main(void) { int r = f(1); printf("f=%d %s\n", r, r == 2 + 'b' + 6 ? "OK" : "BAD"); return 0; }
