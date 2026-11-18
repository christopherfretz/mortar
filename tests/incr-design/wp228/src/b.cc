#include "h.h"
int g_b = 7;
const char *g_msg = "hello from b";
struct BS { int x; int y; };
static int b_helper(int k) { return k + 5; }
int b_f(int x) {
  BS s{x, 2};
  Box<int> bx{x};
  int r = s.x * s.y + bx.get();
  for (int i = 0; i < x; ++i) r += twice(i);
  return b_helper(r) + g_b;
}
