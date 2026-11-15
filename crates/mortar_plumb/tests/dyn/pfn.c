#include <stdio.h>
extern int pfn(void); extern void *libp_pfn_addr(void);
void *volatile take = (void *)&pfn;
int main(void) { void *e = (void *)&pfn; int ok = e == libp_pfn_addr() && take == e;
  printf("protected-fn &pfn exe=%p data=%p lib=%p %s\n", e, take, libp_pfn_addr(), ok ? "PASS" : "FAIL"); return !ok; }
