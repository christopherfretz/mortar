#include <cstdio>
int c_entry(int);
int u_keep(int);
int main() {
  std::printf("c_entry(5)=%d u_keep(5)=%d\n", c_entry(5), u_keep(5));
  return 0;
}
