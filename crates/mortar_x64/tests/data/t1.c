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
static char big[1 << 20];
int use(int i) {
  int r = ext_tls + def_tls + (int)st_tls[i & 3] + hid_tls;
  if (&weak_tls) r += weak_tls;
  r += ext_data + ext_arr[i] + def_data + st_data + hid_data + big[i];
  if (&weak_data) r += weak_data;
  if (weak_fn) weak_fn();
  ext_fn();
  st_tls[i & 3] += r; def_tls = r; ext_tls = i; hid_tls ^= r;
  return r + (ext_data > i ? 1 : 0) + (int)(long)&ext_data;
}
int *addr_tls(void) { return &ext_tls; }
long *addr_st(int i) { return &st_tls[i&3]; }
void (*fp(void))(void) { return ext_fn; }
int cmpaddr(long x) { return x == (long)&ext_data || x < (long)&def_data; }
long andaddr(long x) { return (x & (long)&ext_data) ^ ((long)&ext_arr | x) ; }
