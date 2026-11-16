// dlopened library with a thread_local object with a constructor/destructor
#include <cstdio>
#include <pthread.h>
struct T {
    int v;
    T() : v(100) { std::printf("EV T ctor\n"); std::fflush(stdout); }
    ~T() { std::printf("EV T dtor v=%d\n", v); std::fflush(stdout); }
};
thread_local T t;
__thread int plain_tls = 5;
extern "C" int tls_bump(int by) { t.v += by; plain_tls += by; return t.v * 1000 + plain_tls; }
extern "C" void *tls_addr() { return &t; }
