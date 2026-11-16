#include <stdio.h>
#define D(x) int g_##x(void);
D(dat) D(wd) D(cm) D(tv) D(wtv) D(fn) D(wf) D(nt) D(wnt) D(ifn) D(wifn) D(pd) D(pf) D(ptv)
#define C(x,v) printf(" %s=%s", #x, g_##x() == v ? "P" : "B");
int main(void) { C(dat,101) C(wd,102) C(cm,103) C(tv,104) C(wtv,105) C(fn,106) C(wf,107) C(nt,108) C(wnt,109)
  C(ifn,110) C(wifn,111) C(pd,112) C(pf,113) C(ptv,114) printf("\n"); return 0; }
