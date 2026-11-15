#include <stdio.h>
#include <string.h>
__thread int tv = 5;
static __thread int stv[4] = {1,2,3,4};
extern __thread int etv;
__thread int etv = 7;
__attribute__((weak)) extern __thread int wtv;
__thread char tbuf[100] __attribute__((aligned(64)));
static __thread long long tll;
extern int ext_var;
int ext_var = 3;
__attribute__((weak)) extern int weak_var;
__attribute__((weak)) void weak_fn(void);
static int sw(int x){ switch(x){case 0: return 11; case 1: return 22; case 2: return 33; case 3: return 44; case 4: return 55; case 5: return 66; default: return -1;} }
int (*fp)(const char*, ...) = printf;
size_t (*sl)(const char*) = strlen;
const char *s1 = "hello world";
int getc_(int i){ const char *p = "abcdefgh"; return p[i-1] + "xyz"[i+1]; }
int addr_tests(void){ return (&weak_var != 0) + (weak_fn != 0) + (&wtv != 0)*10; }
int use_tls(int i){ tbuf[i] = (char)i; tll += i; return tv + stv[i&3] + etv + tbuf[i] + (int)tll; }
int *tls_addr(void){ return &tv; }
int main(int argc, char **argv){
  int r = use_tls(argc) + sw(argc) + getc_(argc+1) + ext_var + (int)strlen(s1) + (sl == strlen) + addr_tests();
  fp("%d %s\n", r, argv[0] ? "ok" : "no");
  return 0;
}
