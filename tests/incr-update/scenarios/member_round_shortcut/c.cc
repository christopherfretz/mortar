// The changed file. EDIT: calls m3 directly (a shorter path).
int m1_fn(int);
int m3_fn(int);
#ifdef EDIT
int c_fn() { return m1_fn(1) + m3_fn(1); }
#else
int c_fn() { return m1_fn(1); }
#endif
