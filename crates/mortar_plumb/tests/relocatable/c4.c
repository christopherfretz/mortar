/* c4.c: an IFUNC definition (mold 2.37.1 -r crashes on it). */
static int impl_a(void) { return 40; }
static int (*resolve_f(void))(void) { return impl_a; }
int ifn(void) __attribute__((ifunc("resolve_f")));
