/* A stand-in for libc (soname libc.so.6): the runtime's imports (spec/odr.md O-53). */
long write(int f, const void *b, unsigned long n) { (void)f; (void)b; return (long)n; }
void *mmap(void *a, unsigned long n, int p, int f, int d, long o) { (void)n; (void)p; (void)f; (void)d; (void)o; return a; }
int munmap(void *a, unsigned long n) { (void)a; (void)n; return 0; }
char *getenv(const char *n) { (void)n; return 0; }
unsigned long getauxval(unsigned long t) { (void)t; return 0; }
int clock_gettime(int c, void *t) { (void)c; (void)t; return 0; }
int dl_iterate_phdr(void *cb, void *d) { (void)cb; (void)d; return 0; }
void abort(void) { for (;;) {} }
