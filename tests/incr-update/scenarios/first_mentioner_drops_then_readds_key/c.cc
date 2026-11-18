// The changed file, first in link order. EDIT: no mention of k_value.
#ifndef EDIT
extern int k_value;
int c_fn() { return k_value + 1; }
#else
int c_fn() { return 3; }
#endif
