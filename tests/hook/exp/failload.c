/* WP-077 "fail the load": can an init function make dlopen RETURN an error?
 * -DVIA_SIGNAL: the constructor calls glibc's _dl_signal_error (GLIBC_PRIVATE,
 * what the loader itself uses to fail a load); default: the constructor calls
 * dlclose on its own handle (dlopen(RTLD_NOLOAD)) and returns. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
extern void _dl_signal_error(int errcode, const char* object, const char* occasion, const char* errstring);
int failload_entry(void) { return 7; }
__attribute__((constructor)) static void c(void) {
  fprintf(stderr, "failload: constructor runs\n");
#ifdef VIA_SIGNAL
  _dl_signal_error(0, "failload.so", "odr check", "odr-violation-at-load");
#else
  void* self = dlopen("failload-close.so", RTLD_NOW | RTLD_NOLOAD);
  fprintf(stderr, "failload: own handle %p\n", self);
  if (self) { dlclose(self); dlclose(self); }
#endif
  fprintf(stderr, "failload: constructor returns\n");
}
