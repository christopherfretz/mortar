/* WP-161: a library for the option tail of -shared (D-102, D-103, D-110,
   D-112): exports, a TLS variable read through a descriptor, an
   -init/-fini pair, and a use of an archive member's `foo_fn`. */
int foo_fn(void);
__thread int opt_tls = 4;
int opt_data = 2;
static int inits;
void opt_init(void) { inits++; }
void opt_fini(void) { inits--; }
int opt_fn(void) { return foo_fn() + opt_data + opt_tls + inits; }
int opt_other(void) { return opt_fn() * 2; }
