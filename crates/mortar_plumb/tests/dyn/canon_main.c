#include <stdio.h>
#include <stdlib.h>
#include <dlfcn.h>
void *addr_puts_nopic(void); void *addr_qsort_nopic(void);
extern void *lib_addr_puts(void);
int main(void) {
  void *a = addr_puts_nopic(), *b = (void *)&puts, *c = lib_addr_puts(), *d = dlsym(RTLD_DEFAULT, "puts");
  printf("nopic=%d pie=%d lib=%d dlsym=%d (equal to nopic)\n", 1, a == b, a == c, a == d);
  return !(a == b && a == c && a == d);
}
