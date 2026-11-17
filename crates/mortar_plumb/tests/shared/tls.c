/* WP-144: TLS in an x86-64 shared object, every binding: an exported
   variable (preemptible), a hidden one, two static ones read in one
   function (GCC's local-dynamic: `_TLS_MODULE_BASE_` under gnu2), and
   one imported from another library. Compiled per dialect. */
__thread int e_tls = 1;
__attribute__((visibility("hidden"))) __thread int h_tls = 2;
static __thread int s_tls = 3;
static __thread int s2_tls = 5;
extern __thread int o_tls;
int get_e(void) { return e_tls; }
int get_h(void) { return h_tls; }
int get_s(void) { return s_tls + s2_tls; }
int get_o(void) { return o_tls; }
void bump(int k) {
  e_tls += k;
  h_tls += k;
  s_tls += k;
  s2_tls += 2 * k;
  o_tls += k;
}
