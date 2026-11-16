#include <stdio.h>
#include <dlfcn.h>
extern int libfn(int);
extern int foo_global;
extern __thread int foo_tls;
void after_dlopen(void) { __asm__ volatile("" ::: "memory"); }
int main(void)
{
    int a = libfn(5);       /* HOST_CALL_LINE */
    printf("libfn(5)=%d foo_global=%d foo_tls=%d\n", a, foo_global, foo_tls);
    void *h = dlopen("libplug.so", RTLD_NOW);
    if (!h) { printf("FAIL dlopen %s\n", dlerror()); return 1; }
    after_dlopen();
    int (*p)(int) = (int (*)(int))dlsym(h, "plugfn");
    int b = p(4);
    printf("plugfn(4)=%d\n", b);
    dlclose(h);
    printf("%s\n", (a == 11 && b == 12 && foo_tls == 18) ? "OK host-a" : "FAIL host-a");
    return 0;
}
