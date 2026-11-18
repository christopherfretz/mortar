/* Unchanged: always references w_both, so w2.o stays extracted. */
int w_both(int);
int u_fn(void) { return w_both(10); }
