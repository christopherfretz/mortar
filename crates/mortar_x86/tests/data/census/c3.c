/* corpus source: TLS in all models from one file, plus a definition for c1.c's externs */
__thread int ext_tls = 9;
__thread int tl_g __attribute__((tls_model("global-dynamic"))) = 1;
static __thread int tl_l __attribute__((tls_model("local-dynamic"))) = 2;
static __thread int tl_l2 __attribute__((tls_model("local-dynamic"))) = 3;
extern __thread int tl_ie __attribute__((tls_model("initial-exec")));
__thread int tl_le __attribute__((tls_model("local-exec"))) = 4;
int tls_all(void) { return tl_g + tl_l + tl_l2 + tl_ie + tl_le; }
int *tls_addr(int i) { return i ? &tl_l : &tl_g; }
