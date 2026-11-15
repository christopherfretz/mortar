/* corpus source: every kind of reference a C translation unit makes */
extern __thread int ext_tls;
__thread int def_tls = 3;
static __thread long st_tls[4] __attribute__((aligned(64)));
extern __thread int weak_tls __attribute__((weak));
__attribute__((visibility("hidden"))) __thread int hid_tls;
extern int ext_data; extern int ext_arr[];
int def_data = 5;
static int st_data;
extern int weak_data __attribute__((weak));
extern void ext_fn(void);
extern void weak_fn(void) __attribute__((weak));
__attribute__((visibility("hidden"))) extern int hid_data;
static char big[1 << 17];
int comm_var __attribute__((common));
long double ld_var = 1.5L;
int *init_ptr = &ext_arr[3];          /* data word with an addend */
const char *str_ptr = "merge-me" + 2;  /* section symbol of a merge section plus addend */
static int sel_impl(int x) { return x + 1; }
static int (*resolve_sel(void))(int) { return sel_impl; }
int sel(int) __attribute__((ifunc("resolve_sel")));
int (*sel_ptr)(int) = sel;
int use(int i) {
  int r = ext_tls + def_tls + (int)st_tls[i & 3] + hid_tls;
  if (&weak_tls) r += weak_tls;
  r += ext_data + ext_arr[i] + def_data + st_data + hid_data + big[i] + comm_var;
  if (&weak_data) r += weak_data;
  if (weak_fn) weak_fn();
  ext_fn();
  r += sel(i) + sel_ptr(i);
  st_tls[i & 3] += r; def_tls = r; ext_tls = i; hid_tls ^= r;
  return r + (ext_data > i ? 1 : 0) + (int)(long)&ext_data;
}
int *addr_tls(void) { return &ext_tls; }
long *addr_st(int i) { return &st_tls[i&3]; }
void (*fp(void))(void) { return ext_fn; }
int cmpaddr(long x) { return x == (long)&ext_data || x < (long)&def_data; }
long andaddr(long x) { return (x & (long)&ext_data) ^ ((long)&ext_arr | x) ; }
long long lldiv_(long long a, long long b) { return a / b + a % b; }
double fconst(double x) { return x * 3.25 + 1e-3; }
const char *pick(int i) {
  switch (i) { case 0: return "zero"; case 1: return "one"; case 2: return "two"; case 3: return "three";
               case 4: return "four"; case 5: return "five"; case 6: return "six"; default: return "many"; }
}
