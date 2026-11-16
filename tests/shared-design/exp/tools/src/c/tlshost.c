#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
static int (*bump)(int);
static void *(*addr)(void);
static void *res[2];
static int val[2];
static pthread_barrier_t bar;
static void *thr(void *p)
{
    int i = (int)(long)p;
    int r = 0;
    for (int k = 0; k < 3; k++) r = bump(i + 1);
    val[i] = r; res[i] = addr();
    pthread_barrier_wait(&bar);   /* both threads alive: their TLS blocks are distinct */
    printf("HOST thread %d bump=%d\n", i, r); fflush(stdout);
    return 0;
}
int main(int argc, char **argv)
{
    void *h = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!h) { printf("FAIL %s\n", dlerror()); return 1; }
    bump = (int (*)(int))dlsym(h, "tls_bump");
    addr = (void *(*)(void))dlsym(h, "tls_addr");
    pthread_t a, b;
    pthread_barrier_init(&bar, 0, 2);
    pthread_create(&a, 0, thr, (void *)0L);
    pthread_create(&b, 0, thr, (void *)1L);
    pthread_join(a, 0); pthread_join(b, 0);
    int m = bump(7);
    printf("HOST main bump=%d\n", m);
    /* thread 0: 3 bumps of 1 -> T 103, plain 8 -> 103008; thread 1: by 2 -> 106011 */
    int ok = val[0] == 103008 && val[1] == 106011 && m == 107012 && res[0] != res[1];
    printf("%s tls\n", ok ? "OK" : "FAIL");
    dlclose(h);
    printf("HOST exit\n");
    return 0;
}
