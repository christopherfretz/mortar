// x1.cc: first object of the C++ component.
#include <stdexcept>
#include <string>
inline int shared_inline(int x) { static int k = x; return k + x; }   // COMDAT, with a guard
template <class T> T twice(T v) { return v + v; }                     // COMDAT instantiation
struct Reg { int v; Reg() : v(42) {} };
static Reg reg1;                                                      // static initializer
int x1_value() { return reg1.v + shared_inline(1) + twice(2); }
void x1_throw(int code) {
  if (code > 0) throw std::runtime_error("x1 says " + std::to_string(code));
}
