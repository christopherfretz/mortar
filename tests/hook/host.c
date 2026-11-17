/* WP-077: the matrix's driver (after the re-review's hook-rebind.sh host).
 * ops, in order:
 *   o:PATH:FLAGS  dlopen; FLAGS: g RTLD_GLOBAL (else LOCAL), l RTLD_LAZY (else NOW),
 *                 d RTLD_DEEPBIND, n RTLD_NOLOAD
 *   c:PATH        call plugin_entry of PATH's latest handle
 *   m:PATH        dlmopen(LM_ID_NEWLM) and call its plugin_entry
 *   x:PATH        dlclose PATH's latest handle
 *   k:NAME        call the int(void) function NAME found from the executable (dlsym RTLD_DEFAULT)
 * Built twice: by GCC as a foreign host, by mortar with the runtime as a Mason host. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <string.h>
static struct { char p[512]; void* h; } hs[64];
static int nh;
int main(int argc, char** argv) {
  setvbuf(stdout, 0, _IONBF, 0);
  for (int i = 1; i < argc; i++) {
    char op = argv[i][0];
    char p[512];
    strncpy(p, argv[i] + 2, 511);
    p[511] = 0;
    char* fl = strchr(p, ':');
    if (fl) *fl++ = 0; else fl = "";
    if (op == 'o') {
      int m = strchr(fl, 'l') ? RTLD_LAZY : RTLD_NOW;
      m |= strchr(fl, 'g') ? RTLD_GLOBAL : RTLD_LOCAL;
      if (strchr(fl, 'd')) m |= RTLD_DEEPBIND;
      if (strchr(fl, 'n')) m |= RTLD_NOLOAD;
      void* h = dlopen(p, m);
      if (!h) { printf("dlopen %s: %s\n", p, dlerror()); return 1; }
      printf("opened %s [%s]\n", strrchr(p, '/') ? strrchr(p, '/') + 1 : p, fl);
      strcpy(hs[nh].p, p); hs[nh++].h = h;
    } else if (op == 'c' || op == 'x') {
      int k;
      for (k = nh - 1; k >= 0; k--) if (hs[k].h && !strcmp(hs[k].p, p)) break;
      if (k < 0) { printf("no handle for %s\n", p); return 1; }
      if (op == 'c') {
        int (*f)(void) = (int (*)(void))dlsym(hs[k].h, "plugin_entry");
        printf("call %s -> %d\n", strrchr(p, '/') ? strrchr(p, '/') + 1 : p, f());
      } else {
        dlclose(hs[k].h); hs[k].h = 0; printf("closed %s\n", strrchr(p, '/') ? strrchr(p, '/') + 1 : p);
      }
    } else if (op == 'm') {
      void* h = dlmopen(LM_ID_NEWLM, p, RTLD_NOW);
      if (!h) { printf("dlmopen %s: %s\n", p, dlerror()); return 1; }
      int (*f)(void) = (int (*)(void))dlsym(h, "plugin_entry");
      printf("new namespace %s -> %d\n", strrchr(p, '/') ? strrchr(p, '/') + 1 : p, f());
    } else if (op == 'k') {
      int (*f)(void) = (int (*)(void))dlsym(RTLD_DEFAULT, p);
      printf("%s -> %d\n", p, f ? f() : -1);
    }
  }
  return 0;
}
