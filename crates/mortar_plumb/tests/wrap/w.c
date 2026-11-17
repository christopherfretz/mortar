int __real_f(void);
int __wrap_f(void) { return 100 + __real_f(); }
