/* The second library of tests/shared.rs: initial-exec TLS (DF_STATIC_TLS),
   a preemptible IFUNC (W20, W21), `__start_`/`__stop_` of a C-identifier
   section (D-110 item 2), `-init`/`-fini` functions (D-124) and an
   archive member hidden by `--exclude-libs` (D-114). */
__attribute__((tls_model("initial-exec"))) __thread int ietv = 3;
int get_ie(void) { return ietv; }

static int impl1(void) { return 1; }
static int (*resolve_ifn(void))(void) { return impl1; }
int ifn(void) __attribute__((ifunc("resolve_ifn")));
int (*ifn_p)(void) = ifn;
int call_ifn(void) { return ifn(); }

__attribute__((section("cid_ref"), used)) int cid_a = 1;
extern int __start_cid_ref[], __stop_cid_ref[];
long cid_count(void) { return __stop_cid_ref - __start_cid_ref; }

int inits;
void myinit(void) { inits++; }
void myfini(void) { inits--; }

extern int arfn(void);
int use_ar(void) { return arfn(); }
