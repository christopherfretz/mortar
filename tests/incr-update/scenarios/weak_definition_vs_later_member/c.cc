// The changed file. EDIT: calls k_fn (extracting m.o from lib.a).
int k_fn();
#ifdef EDIT
int c_fn() { return k_fn() + 1; }
#else
int c_fn() { return 1; }
#endif
