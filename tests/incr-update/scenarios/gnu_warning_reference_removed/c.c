/* The changed file. EDIT: no reference to either warned symbol. */
#ifndef EDIT
int w_once(int);
int w_both(int);
int c_fn(void) { return w_once(1) + w_both(2); }
#else
int c_fn(void) { return 3; }
#endif
