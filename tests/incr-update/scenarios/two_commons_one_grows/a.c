/* The changed file. EDIT: buf is 256 bytes. */
#ifdef EDIT
char buf[256];
#else
char buf[4];
#endif
int a_size(void) { return (int)sizeof buf; }
