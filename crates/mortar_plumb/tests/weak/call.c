extern int vfn(void) __attribute__((weak));
int ccall(void) { return vfn(); }
