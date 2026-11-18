// The changed file. EDIT: no call into m1.
int m1_fn(int);
#ifndef EDIT
int d_fn() { return m1_fn(3); }
#else
int d_fn() { return 0; }
#endif
