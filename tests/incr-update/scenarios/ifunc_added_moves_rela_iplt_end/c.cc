// The changed file. EDIT: an IFUNC, called.
#ifdef EDIT
extern "C" int c_impl() { return 5; }
extern "C" void *c_resolve() { return (void *)c_impl; }
extern "C" int c_ifunc() __attribute__((ifunc("c_resolve")));
int c_fn() { return c_ifunc(); }
#else
int c_fn() { return 0; }
#endif
