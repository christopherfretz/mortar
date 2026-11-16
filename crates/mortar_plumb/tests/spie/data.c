/* Address words: data pointers, .init_array/.fini_array, a const table. */
#include <stdio.h>
int a = 1, b = 2;
int *pa = &a;
int *const cpb = &b;
static void hi(void){ puts("ctor"); }
static void bye(void){ puts("dtor"); }
void (*init_p)(void) __attribute__((section(".init_array"), used)) = hi;
void (*fini_p)(void) __attribute__((section(".fini_array"), used)) = bye;
const char *names[] = {"x", "y", "z"};
int main(void){ printf("%d %d %s\n", *pa, *cpb, names[2]); return 0; }
