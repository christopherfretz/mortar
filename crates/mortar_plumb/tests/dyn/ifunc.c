/* ifunc defined (with its resolver) in the executable; resolver has no relocation dependencies */
static int impl1(void) { return 11; }
int (*ifn_resolver(void))(void) { return impl1; }
int ifn(void) __attribute__((ifunc("ifn_resolver")));
void *ifn_addr_other_tu(void) { return (void *)&ifn; }
