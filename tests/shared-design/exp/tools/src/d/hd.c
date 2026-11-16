/* Interposition hosts. -DDEF: the executable defines f (2) and v (20).
   -DDL: L is dlopened (argv[1] = path of L, argv[2] = mode words: local,
   global, deepbind; argv[3] optional: a library M to dlopen first, with
   argv[4] its mode); otherwise L is a DT_NEEDED library.
   -DEXTV: the executable writes L's v (extern) before asking L. */
#include <dlfcn.h>
#include <stdio.h>
#include <string.h>
#ifdef DEF
int f(void) { return 2; }
int v = 20;
#endif
#ifndef DL
extern int L_calls_f(void), L_reads_v(void);
extern void *L_addr_f(void), *L_addr_v(void);
#ifndef DEF
extern int f(void);
extern int v;
#endif
#endif
static int mode_of(const char *s)
{
    int m = RTLD_NOW;
    m |= strstr(s, "global") ? RTLD_GLOBAL : RTLD_LOCAL;
    if (strstr(s, "deepbind")) m |= RTLD_DEEPBIND;
    return m;
}
int main(int argc, char **argv)
{
    int (*cf)(void), (*rv)(void);
    void *(*af)(void), *(*av)(void);
#ifdef DL
    if (argc > 4) {
        void *hm = dlopen(argv[3], mode_of(argv[4]));
        if (!hm) { printf("FAIL dlopen M %s\n", dlerror()); return 1; }
    }
    void *h = dlopen(argv[1], mode_of(argv[2]));
    if (!h) { printf("FAIL dlopen L %s\n", dlerror()); return 1; }
    cf = (int (*)(void))dlsym(h, "L_calls_f");
    rv = (int (*)(void))dlsym(h, "L_reads_v");
    af = (void *(*)(void))dlsym(h, "L_addr_f");
    av = (void *(*)(void))dlsym(h, "L_addr_v");
    void *gf = dlsym(RTLD_DEFAULT, "f"), *lf = dlsym(h, "f");
#else
    cf = L_calls_f; rv = L_reads_v; af = L_addr_f; av = L_addr_v;
#ifdef EXTV
    v = 99;
#endif
    void *gf = (void *)&f, *lf = (void *)&f;
#endif
    printf("RESULT L.f()->%d L.v->%d", cf(), rv());
#ifndef DL
    printf(" exe&f==L&f:%d exe&v==L&v:%d", (void *)&f == af(), (void *)&v == av());
#else
    printf(" dlsym(DEFAULT,f)==L&f:%d dlsym(L,f)==L&f:%d", gf == af(), lf == af());
#endif
    printf("\n");
    return 0;
}
