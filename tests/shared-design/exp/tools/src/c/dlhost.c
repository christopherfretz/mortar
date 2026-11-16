/* dlopen/dlsym/dlclose/dlopen again, in the mode given by argv[2] */
#include <dlfcn.h>
#include <stdio.h>
#include <string.h>
extern __attribute__((visibility("hidden"))) void *__dso_handle;
static void hp(const char *s) { printf("HOST %s\n", s); fflush(stdout); }
int main(int argc, char **argv)
{
    const char *lib = argv[1];
    int mode = 0;
    mode |= strstr(argv[2], "lazy") ? RTLD_LAZY : RTLD_NOW;
    mode |= strstr(argv[2], "global") ? RTLD_GLOBAL : RTLD_LOCAL;
    for (int round = 1; round <= 2; round++) {
        char b[64];
        snprintf(b, sizeof b, "dlopen round %d", round); hp(b);
        void *h = dlopen(lib, mode);
        if (!h) { printf("FAIL dlopen %s\n", dlerror()); return 1; }
        int (*f)(void) = (int (*)(void))dlsym(h, "cyc_call");
        if (!f) { printf("FAIL dlsym %s\n", dlerror()); return 1; }
        int r = f();
        snprintf(b, sizeof b, "cyc_call returned %d", r); hp(b);
        void *(*dh)(void) = (void *(*)(void))dlsym(h, "lib_dso_handle");
        if (dh) {
            void *l = dh();
            printf("HOST dso_handle %s\n", (l != (void *)&__dso_handle && l != 0) ? "distinct" : "SAME-OR-NULL");
        }
        hp("dlclose");
        if (dlclose(h)) { printf("FAIL dlclose %s\n", dlerror()); return 1; }
        hp("dlclose returned");
    }
    hp("exit");
    return 0;
}
