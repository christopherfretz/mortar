int f(void);
int g(void);
int __real_f(void);
int r;
void _start(void) { r = f() + g() + __real_f(); for (;;) ; }
