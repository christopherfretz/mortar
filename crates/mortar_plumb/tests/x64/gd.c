/* W4: GCC -fPIC -mcmodel=large general-dynamic (the 22-byte form). */
#include <stdio.h>
__thread int ga = 3;
__thread long gb[4] = {1,2,3,4};
__attribute__((noinline)) int get(int i){ return ga + (int)gb[i]; }
int main(void){ printf("GD ok %d %d\n", get(0), get(3)); return 0; }
