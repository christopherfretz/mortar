#include <stdio.h>
extern int g; int all(void); void set(int);
int main(void) { g = 100; int a = all(); set(7); printf("all=%d g=%d %s\n", a, g, (a == 107 && g == 7) ? "OK" : "MISLINK"); return 0; }
