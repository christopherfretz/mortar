int f(void) { return 1; }
int d = 1;
__thread int t = 1;
static int iimpl(void) { return 1; }
static void *ires(void) { return (void *)iimpl; }
int i(void) __attribute__((ifunc("ires")));
extern int n;
__asm__(".globl n\n.data\n.p2align 2\nn: .long 1\n.text\n");
int probe(void) { return (f() == 100) | (d == 100) << 1 | (n == 100) << 2 | (t == 100) << 3 | (i() == 100) << 4; }
