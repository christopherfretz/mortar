#include <cstring>
#include "h.hh"
int c_f(int x) {
  Point q{WP249_TWICE(x), 3};
  Box<int> bx{q.x};
  return bx.get() + (int)std::strlen("c");
}
