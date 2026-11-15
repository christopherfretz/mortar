// Static constructors and destructors: order within a TU, init_priority,
// constructor/destructor attributes with priorities, atexit, function-local
// statics, and destruction in reverse order.
#include <cstdio>
#include <cstdlib>
struct Rec {
    const char *n;
    explicit Rec(const char *s) : n(s) { std::printf("ctor %s\n", n); }
    ~Rec() { std::printf("dtor %s\n", n); }
};
Rec a1("a1");
Rec a2("a2");
__attribute__((init_priority(200))) Rec early("early-200");
__attribute__((init_priority(1000))) Rec mid("mid-1000");
__attribute__((constructor(150))) static void c150() { std::printf("constructor 150\n"); }
__attribute__((constructor)) static void cdef() { std::printf("constructor default\n"); }
__attribute__((destructor(150))) static void d150() { std::printf("destructor 150\n"); }
__attribute__((destructor)) static void ddef() { std::printf("destructor default\n"); }
static void at1() { std::printf("atexit 1\n"); }
static void at2() { std::printf("atexit 2\n"); }
Rec &lazy() { static Rec r("lazy"); return r; }
int register_atexit() { std::atexit(at1); std::atexit(at2); return 0; }
