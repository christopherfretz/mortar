#include <stdio.h>
extern long res[]; extern const char *names[]; extern int run_checks(void);
__thread long other_tls = 1;
int main(void) { int n = run_checks(), bad = 0;
  for (int i = 0; i < n; i++) if (res[2*i] != res[2*i+1]) { bad++; printf("FAIL %s got %lx want %lx\n", names[i], res[2*i], res[2*i+1]); }
  printf("%d checks, %d failures\n", n, bad); return bad != 0; }
