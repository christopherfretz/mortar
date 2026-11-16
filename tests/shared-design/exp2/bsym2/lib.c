/* bsym2: one definition per (type x binding x visibility) cell of D-112. */
#define P __attribute__((visibility("protected")))
int dat = 1;                          /* OBJECT global */
__attribute__((weak)) int wd = 2;     /* OBJECT weak */
int cm;                               /* COMMON (-fcommon); 0 */
__thread int tv = 4;                  /* TLS global */
__attribute__((weak)) __thread int wtv = 5;  /* TLS weak */
int fn(void) { return 6; }            /* FUNC global */
__attribute__((weak)) int wf(void) { return 7; }   /* FUNC weak */
#ifdef __aarch64__
__asm__(".text\n.globl nt\n.type nt,%notype\n.p2align 2\nnt: mov w0, #8\n ret\n"
        ".globl wnt\n.weak wnt\n.p2align 2\nwnt: mov w0, #9\n ret\n");
#else
__asm__(".text\n.globl nt\nnt: movl $8, %eax\n ret\n.weak wnt\nwnt: movl $9, %eax\n ret\n");
#endif
static int impl(void) { return 10; }
static void *res(void) { return (void *)impl; }
int ifn(void) __attribute__((ifunc("res")));                 /* GNU_IFUNC global */
int wifn(void) __attribute__((ifunc("res"))); __asm__(".weak wifn");   /* GNU_IFUNC weak (GCC refuses weak+ifunc) */
P int pd = 12;                        /* OBJECT protected */
P int pf(void) { return 13; }         /* FUNC protected */
P __thread int ptv = 14;              /* TLS protected */
extern int nt(void), wnt(void);
int g_dat(void){return dat;} int g_wd(void){return wd;} int g_cm(void){return cm;}
int g_tv(void){return tv;} int g_wtv(void){return wtv;}
int g_fn(void){return fn();} int g_wf(void){return wf();} int g_nt(void){return nt();} int g_wnt(void){return wnt();}
int g_ifn(void){return ifn();} int g_wifn(void){return wifn();}
int g_pd(void){return pd;} int g_pf(void){return pf();} int g_ptv(void){return ptv;}
