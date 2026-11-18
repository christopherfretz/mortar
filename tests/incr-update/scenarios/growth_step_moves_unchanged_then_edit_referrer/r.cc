// REF: the referrer of u.o, edited after the growth step.
int u_fn(int);
#ifdef REF
int r_fn() { return u_fn(3) + 100; }
#else
int r_fn() { return u_fn(3); }
#endif
