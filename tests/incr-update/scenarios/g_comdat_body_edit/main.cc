#include <cstdio>
#include "t.h"
int c_value(int);
int main() {
  std::printf("c_value=%d twice=%d scale=%d\n", c_value(2), twice(4), scale(5));
  return 0;
}
