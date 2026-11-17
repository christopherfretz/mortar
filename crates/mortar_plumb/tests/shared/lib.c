/* A library with one key of each kind D-110 and D-112 tell apart (WP-121). */
extern int imp(void);
extern int impdat;
extern int weakimp(void) __attribute__((weak));

int dat = 1;
int dat2 = 5;
__attribute__((visibility("protected"))) int pdat = 2;
__attribute__((visibility("hidden"))) int hdat = 3;
static int sdat = 4;
__thread int tv = 7;
__attribute__((visibility("hidden"))) __thread int htv = 8;

__attribute__((weak)) int wfn(void) { return 1; }
__attribute__((noinline)) int fn(void) { return dat + pdat + hdat + sdat + tv + htv; }
__attribute__((visibility("protected"), noinline)) int pfn(void) { return dat2; }

int *dat_p = &dat;
int *pdat_p = &pdat;
int (*fn_p)(void) = fn;
int (*imp_p)(void) = imp;
int *impdat_p = &impdat;

int caller(void) {
  return fn() + wfn() + pfn() + imp() + impdat + (weakimp ? weakimp() : 0) + *dat_p;
}
