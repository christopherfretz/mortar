extern int vfn(void) __attribute__((weak));
void *gaddr(void) { return (void *)&vfn; }
