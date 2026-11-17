/* W24: a .ctors input section. */
int w24_ran;
static void w24_ctor(void) { w24_ran = 1; }
__attribute__((section(".ctors"), used)) static void (*w24_p)(void) = w24_ctor;
