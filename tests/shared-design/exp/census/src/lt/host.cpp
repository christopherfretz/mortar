// host for the libtool-shaped libraries (linked by GNU ld)
#include <cstdio>
#include <stdexcept>
#include <dlfcn.h>
extern "C" { int foo(void); int conv(void); int dep(void); }
const char *bar_ctor();
int bar(int);
int main() {
  std::setvbuf(stdout, nullptr, _IONBF, 0);
  std::printf("%s foo=%d\n", foo() == 42 ? "OK" : "FAIL", foo());
  std::printf("%s bar(21)=%d ctor=%s\n", bar(21) == 42 && bar_ctor()[0] == 'y' ? "OK" : "FAIL", bar(21), bar_ctor());
  try { bar(-1); std::printf("FAIL no throw\n"); } catch (const std::runtime_error &e) { std::printf("OK caught: %s\n", e.what()); }
  std::printf("%s conv=%d\n", conv() == 7 ? "OK" : "FAIL", conv());
  std::printf("%s dep=%d\n", dep() == 43 ? "OK" : "FAIL", dep());
  void *h = dlopen("libfoo.so.1", RTLD_NOW | RTLD_NOLOAD);
  std::printf("%s foo_hidden_by_script %s\n", h && !dlsym(h, "foo_hidden_by_script") ? "OK" : "FAIL",
              h && dlsym(h, "foo_hidden_by_script") ? "exported" : "hidden");
  return 0;
}
