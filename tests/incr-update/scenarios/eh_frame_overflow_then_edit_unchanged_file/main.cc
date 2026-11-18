#include <cstdio>
int big(int);
int later(int);
int main() {
  std::printf("big(0)=%d later(3)=%d later(4)=%d later(5)=%d\n", big(0), later(3), later(4), later(5));
  return 0;
}
