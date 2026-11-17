int __real_f(void);
int r;
void _start(void) { r = __real_f(); for (;;) ; }
