#include <cstdio>
int c_use(int);
int u_use(int);
int main() {
  std::printf("c_use=%d u_use=%d\n", c_use(2), u_use(3));
  return 0;
}
