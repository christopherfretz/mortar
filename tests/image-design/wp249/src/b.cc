#include <cstdio>
#include "h.hh"
int g_b = 7;
int b_f(int x) {
  Point q{x, WP249_ANSWER};
  Box<long> bl{q.y};
  std::puts("b");
  return twice(q.x) + (int)bl.get();
}
