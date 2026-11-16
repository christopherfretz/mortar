#ifdef __OPTIMIZE__
int last_fn(int x){ return x * 3 + (x >> 2) + (x ^ 9) + 1; }
#else
int last_fn(int x){ return x * 3 + (x ^ 9) + 1; }
#endif
