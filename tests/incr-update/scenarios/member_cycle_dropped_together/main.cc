#include <cstdio>
int g_ctor = 1;
int d_fn();
int main() {
  std::printf("d_fn=%d g_ctor=%d\n", d_fn(), g_ctor);
  return 0;
}
