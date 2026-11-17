void *malloc(unsigned long);
void *__real_malloc(unsigned long);
void *__wrap_malloc(unsigned long n) { return __real_malloc(n); }
void *(*p)(unsigned long) = malloc;
void *(*q)(unsigned long);
void *r;
void _start(void) { q = malloc; r = malloc(1); for (;;) ; }
void __wrap__start(void) { for (;;) ; }
