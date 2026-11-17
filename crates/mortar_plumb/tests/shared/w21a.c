/* W21: one hidden IFUNC used from two translation units (AArch64): its
   address taken in data and in code in each (dyn/partD.py's h_hid). */
static int one(void) { return 1; }
static int (*pick(void))(void) { return one; }
__attribute__((visibility("hidden"))) int hif(void) __attribute__((ifunc("pick")));
int (*w21_pa)(void) = hif;
void *w21_aa(void) { return (void *)hif; }
int w21_ca(void) { return hif(); }
