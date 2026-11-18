#include <cstdio>
extern int g_count;
int a_fn();
int main() {
  std::printf("g_count=%d a_fn=%d\n", g_count, a_fn());
  return 0;
}
