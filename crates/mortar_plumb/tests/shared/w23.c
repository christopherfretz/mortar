/* W23: a C-identifier section reached only through __start_, its
   symbols hidden by a version script, under --gc-sections. */
__attribute__((section("cid_gc"))) int cid_b = 1;
extern int __start_cid_gc[];
int *w23_first(void) { return __start_cid_gc; }
