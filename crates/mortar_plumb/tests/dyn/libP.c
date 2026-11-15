/* protected-visibility definitions */
__attribute__((visibility("protected"))) int pvar = 3;
__attribute__((visibility("protected"))) int pfn(void) { return 5; }
void *libp_pvar_addr(void) { return &pvar; }
void *libp_pfn_addr(void) { return (void *)&pfn; }
int libp_get_pvar(void) { return pvar; }
