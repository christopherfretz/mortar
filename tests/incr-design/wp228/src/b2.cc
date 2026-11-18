// edited: one line more, a body that grew, a struct with one more field,
// a new string
#include "h.h"
int g_b = 9;
const char *g_msg = "hello from the edited b";
struct BS { int x; int y; int z; };
static int b_helper(int k) { return k + 5; }
int b_f(int x) {
  BS s{x, 2, 3};
  Box<int> bx{x};
  int r = s.x * s.y + bx.get();
  r += s.z;
  for (int i = 0; i < x; ++i) r += twice(i);
  return b_helper(r) + g_b;
}
