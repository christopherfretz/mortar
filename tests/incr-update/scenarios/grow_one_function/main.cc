#include <cstdio>
int c_value(int);
int c_other(int);
int u_value(int);
int main() {
  std::printf("c_value=%d c_other=%d u_value=%d\n", c_value(2), c_other(5), u_value(4));
  return 0;
}
