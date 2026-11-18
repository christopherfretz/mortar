#include "h.hh"
#define WP250_A_ONLY 1
int main() {
  Box<int> b{1};
  return b.get() + b_f(wp250_twice(2)) + c_f(3) + WP250_A_ONLY;
}
