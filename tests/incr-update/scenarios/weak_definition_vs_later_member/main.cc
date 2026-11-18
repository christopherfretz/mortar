#include <cstdio>
int c_fn();
int w_fn();
int main() {
  std::printf("c_fn=%d w_fn=%d\n", c_fn(), w_fn());
  return 0;
}
