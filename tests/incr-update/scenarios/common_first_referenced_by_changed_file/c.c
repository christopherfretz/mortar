/* The changed file. EDIT: refers to u.o's common. */
#ifdef EDIT
extern int u_common;
int c_fn(void) { u_common += 5; return u_common; }
#else
int c_fn(void) { return 2; }
#endif
