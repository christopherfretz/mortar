/* local ifunc; address taken in the defining TU (PC-relative, no GOT) */
static int impl(void) { return 7; }
static int (*res(void))(void) { return impl; }
int ifq(void) __attribute__((ifunc("res")));
void *ifq_addr_a(void) { return (void *)&ifq; }
