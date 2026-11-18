#include <cstdio>
#include "h.hh"
#define WP249_A_ONLY 1 /* a macro of a.c only: not defined in b */
int main() {
  Point p{1, 2};
  Box<int> bx{p.x};
  int r = b_f(bx.get()) + c_f(twice((int)p.y));
  std::printf("%d %d %d\n", r, g_b, WP249_TWICE(WP249_ANSWER));
  return 0;
}
