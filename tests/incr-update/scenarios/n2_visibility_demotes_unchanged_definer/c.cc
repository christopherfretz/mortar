// The changed file. EDIT: foo declared hidden.
#ifdef EDIT
__attribute__((visibility("hidden")))
#endif
int foo(int);
int c_fn() { return foo(1) + 1; }
