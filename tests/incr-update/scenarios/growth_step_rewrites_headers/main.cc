#include <cstdio>
int grow(int);
int u_fn(int);
extern int u_data;
int main() {
  std::printf("grow=%d u_fn=%d u_data=%d\n", grow(1), u_fn(5), u_data);
  return 0;
}
