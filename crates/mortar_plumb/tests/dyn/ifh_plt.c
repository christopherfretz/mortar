/* ifunc resolver depending on a plt relocation of the executable (classic IRELATIVE ordering hazard) */
#include <stdio.h>
extern int fr(void);
static int good(void) { return 11; }
static int bad(void) { return 22; }
int (*ifh_resolver(void))(void) { return (fr() == 1) ? good : bad; }
int ifh(void) __attribute__((ifunc("ifh_resolver")));
int main(void) { setvbuf(stdout, NULL, _IONBF, 0); int r = ifh(); printf("ifunc-hazard-plt ifh()=%d %s\n", r, r == 11 ? "PASS" : "FAIL"); return r != 11; }
