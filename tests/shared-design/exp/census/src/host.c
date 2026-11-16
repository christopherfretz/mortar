/* host: dlopen argv[1], call lib_check, print OK/FAIL */
#include <dlfcn.h>
#include <stdio.h>
int main(int argc, char **argv) {
  setvbuf(stdout, NULL, _IONBF, 0);
  void *h = dlopen(argv[1], RTLD_NOW);
  if (!h) { printf("FAIL dlopen: %s\n", dlerror()); return 1; }
  int (*f)(void) = (int (*)(void))dlsym(h, "lib_check");
  if (!f) { printf("FAIL dlsym lib_check: %s\n", dlerror()); return 1; }
  int *pi = (int *)dlsym(h, "lib_inited");
  int v = f();
  printf("%s lib_check=%d init=%s ar_func=%s lib_func=%s\n", v == 42 ? "OK" : "FAIL", v,
         pi ? (*pi ? "1" : "0") : "hidden", dlsym(h, "ar_func") ? "exported" : "no",
         dlsym(h, "lib_func") ? "exported" : "no");
  return v == 42 ? 0 : 1;
}
