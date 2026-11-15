/* C corpus unit: every symbol kind, every TLS model, IFUNC, switch tables. */
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
extern int ext_data; extern int ext_arr[];
extern void ext_fn(int);
extern __thread int ext_tls;            /* GD/IE across units */
static __thread int st_tls = 7;         /* LD/LE */
__thread int gl_tls[4] = {1,2,3,4};
__thread char big_tbss[300];
__attribute__((tls_model("initial-exec"))) extern __thread int ie_tls;
__attribute__((tls_model("local-exec"))) __thread long le_tls = 9;
extern int weak_undef __attribute__((weak));
extern void weak_fn(void) __attribute__((weak));
extern __thread int weak_tls __attribute__((weak));
int common_var;
static int local_arr[1000];
const char *strs[] = {"alpha", "beta", "gamma"};
int (*fp)(const char *, ...) = printf;
__attribute__((visibility("hidden"))) int hidden_var = 3;
__attribute__((visibility("protected"))) int prot_var = 4;
static int impl_a(int x){ return x+1; }
static int impl_b(int x){ return x+2; }
/* Static: the resolver runs before libc's own IFUNCs are resolved, so it
   must not call into libc (the review's `getenv` crashes there). */
volatile int pick_b;
static void *resolve_sel(void){ return pick_b ? (void*)impl_b : (void*)impl_a; }
int sel(int) __attribute__((ifunc("resolve_sel")));
int (*sel_ptr)(int) = sel;
int sw(int k){ switch(k){case 0:return 11;case 1:return 23;case 2:return 37;case 3:return 41;case 4:return 59;case 5:return 61;case 6:return 73;default:return -1;} }
__attribute__((noinline)) int *tls_addr(void){ return &st_tls; }
int main(int argc, char **argv){
  char buf[128]; memcpy(buf, argv[0], 4); buf[4]=0;
  local_arr[argc] = argc; common_var += local_arr[1];
  ext_fn(ext_data + ext_arr[1]);
  if (&weak_undef) printf("weak %d\n", weak_undef);
  if (weak_fn) weak_fn();
  if (&weak_tls) puts("wt"); /* X-18: the address only; its value is meaningless */
  st_tls++; gl_tls[argc&3] += ext_tls; big_tbss[argc] = 1; ie_tls += 2; le_tls += 3;
  printf("%s %d %d %d %d %ld %d %d %d %s %d\n", buf, st_tls, gl_tls[1], ext_tls, ie_tls, le_tls, sel(argc), sel_ptr(1), sw(argc), strs[argc%3], *tls_addr()+hidden_var+prot_var);
  fp("%zu\n", sizeof(local_arr));
  return 0;
}
