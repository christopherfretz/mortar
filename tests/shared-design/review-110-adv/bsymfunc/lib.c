int dat = 1;
__thread int tv = 2;
int fn(void){return 3;}
__asm__(".text\n.globl nt\n.p2align 2\nnt: mov w0, #4\n ret\n");
static int impl(void){return 5;}
static void *res(void){return (void*)impl;}
int ifn(void) __attribute__((ifunc("res")));
int wd __attribute__((weak)) = 6;
__attribute__((weak)) int wf(void){return 7;}
int get_dat(void){return dat;}
int get_tv(void){return tv;}
int call_fn(void){return fn();}
extern int nt(void); int call_nt(void){return nt();}
int call_ifn(void){return ifn();}
int get_wd(void){return wd;}
int call_wf(void){return wf();}
