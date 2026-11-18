#include <cstdio>
#include "h.h"
int main(int argc, char **) {
  Box<int> bx{argc};
  int r = b_f(bx.get());
  r += c_f(twice(r));
  Shape s;
  std::printf("%d %d %d %s\n", r, g_b, s.area(), g_msg);
  return 0;
}
