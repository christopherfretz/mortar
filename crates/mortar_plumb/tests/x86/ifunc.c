/* W5: &f of a hidden IFUNC under -fPIE is a GOTOFF; GNU ld writes the
   IPLT entry − 12 in a static link. Also IRELATIVE and the IPLT. */
static int impl(void) { return 0x22; }
static void *resolve(void) { return (void *)impl; }
int f(void) __attribute__((ifunc("resolve")));
int (*pdef)(void) = f;
