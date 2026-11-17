/* W21: one hidden IFUNC used from two translation units (AArch64). */
static int one(void) { return 1; }
static int (*pick(void))(void) { return one; }
__attribute__((visibility("hidden"))) int hif(void) __attribute__((ifunc("pick")));
int (*w21_pa)(void) = hif;
int w21_ca(void) { return hif(); }
