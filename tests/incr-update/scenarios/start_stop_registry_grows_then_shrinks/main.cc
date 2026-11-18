#include <cstdio>
extern const int __start_reg[], __stop_reg[];
int a_fn();
static int sum() { int s = 0; for (const int *p = __start_reg; p < __stop_reg; ++p) s += *p; return s; }
int main() {
  std::printf("entries=%d sum=%d a_fn=%d\n", (int)(__stop_reg - __start_reg), sum(), a_fn());
  return 0;
}
