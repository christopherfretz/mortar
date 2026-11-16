// C++ lifecycle library: static objects (dtor via __cxa_atexit with this
// library's __dso_handle, run by __cxa_finalize from __do_global_dtors_aux),
// constructor/destructor attributes, DT_INIT/DT_FINI (-init/-fini).
#include <cstdio>
static int calls;
static void ev(const char *s) { std::printf("EV %s\n", s); std::fflush(stdout); }
extern "C" void lib_init() { ev("DT_INIT lib_init"); }
extern "C" void lib_fini() { ev("DT_FINI lib_fini"); }
struct S {
    const char *n;
    explicit S(const char *x) : n(x) { std::printf("EV S ctor %s\n", n); std::fflush(stdout); }
    ~S() { std::printf("EV S dtor %s\n", n); std::fflush(stdout); }
};
S s1("s1");
__attribute__((constructor)) static void ctor_attr() { ev("ctor_attr"); }
__attribute__((destructor)) static void dtor_attr() { ev("dtor_attr"); }
S s2("s2");
static S &local_static() { static S s3("s3-function-local"); return s3; }
extern "C" void *lib_dso_handle();
extern __attribute__((visibility("hidden"))) void *__dso_handle;  // C++ declares it implicitly
extern "C" void *lib_dso_handle() { return &__dso_handle; }
extern "C" int cyc_call() { ev("cyc_call"); local_static(); return ++calls; }
