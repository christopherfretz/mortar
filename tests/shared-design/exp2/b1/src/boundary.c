/* B-1 boundary cases, C. */
static int before[4] = {1, 2, 3, 4};
int after = 5;                                  /* exported, right after a local array */
int *one_past = &before[4];                     /* data word: one past the end of 'before' == &after */
int *addr_after = &after;                       /* the exported datum, by symbol */
int get_after(void) { return after; }
int *get_one_past(void) { return &before[4]; }
__asm__(".data\n.globl asmlabel\nasmlabel: .long 7\n.text\n");  /* st_size 0 data definition */
extern int asmlabel;
int get_asm(void) { return asmlabel; }
static int lone_static = 3;
int exported_unused_here = 11;                  /* defined, never used in this TU */
int get_static(void) { return lone_static; }
__thread int tls_exported = 4;
static __thread int tls_static = 5;
int get_tls(void) { return tls_exported + tls_static; }
static int s_alias_target(void) { return 9; }
int exported_fn(void) __attribute__((alias("s_alias_target")));   /* exported alias of a static function */
int call_exported_fn(void) { return exported_fn() + s_alias_target(); }
