#include <cstdio>
extern int k_value;
int c_fn();
int main() {
  std::printf("c_fn=%d k=%d\n", c_fn(), k_value);
  return 0;
}
