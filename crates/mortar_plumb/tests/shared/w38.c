/* W38: the address of an imported function from -fno-pic code. */
extern int imp(void);
void *w38_addr(void) { return (void *)imp; }
