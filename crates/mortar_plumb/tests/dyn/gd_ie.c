#include <stdio.h>
extern __thread int tv;
int *lib_tv(void);
int main(void) { printf("tv %d same %d\n", tv, &tv == lib_tv()); return 0; }
