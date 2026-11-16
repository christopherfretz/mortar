cat > lib.c <<'E'
int f(void) { return 1; }
int d = 1;
__thread int t = 1;
static int iimpl(void) { return 1; }
static void *ires(void) { return (void *)iimpl; }
int i(void) __attribute__((ifunc("ires")));
extern int n;
__asm__(".globl n\n.data\n.p2align 2\nn: .long 1\n.text\n");
int probe(void) { return (f() == 100) | (d == 100) << 1 | (n == 100) << 2 | (t == 100) << 3 | (i() == 100) << 4; }
E
cat > pre.c <<'E'
int f(void) { return 100; }
int d = 100;
int n = 100;
__thread int t = 100;
int i(void) { return 100; }
E
cat > host.c <<'E'
#include <stdio.h>
int probe(void);
int main(void) { int m = probe(); const char *k[] = {"FUNC","OBJECT","NOTYPE","TLS","IFUNC"};
  for (int b = 0; b < 5; b++) printf("%s=%c ", k[b], (m >> b) & 1 ? 'P' : 'B'); printf("\n"); return 0; }
E
echo "{ f; };" > dl.list
