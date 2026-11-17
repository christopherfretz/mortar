extern int vfn(void) __attribute__((weak));
void *taddr(void) { return (void *)&vfn; }
