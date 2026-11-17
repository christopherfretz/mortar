int f(void);
int __wrap_f(void) { return 50; }
int r;
void _start(void) { r = f(); for (;;) ; }
