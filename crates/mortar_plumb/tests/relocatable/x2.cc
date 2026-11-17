// x2.cc: second object of the C++ component: catches what x1 throws.
#include <stdexcept>
#include <cstdio>
inline int shared_inline(int x) { static int k = x; return k + x; }
template <class T> T twice(T v) { return v + v; }
void x1_throw(int);
int x2_catch(int code) {
  try { x1_throw(code); } catch (const std::runtime_error &e) { std::printf("caught: %s\n", e.what()); return 1; }
  return 0;
}
int x2_value() { return shared_inline(5) + twice(10); }
