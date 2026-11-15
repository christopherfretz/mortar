// corpus source: C++ exceptions, thread_local with dynamic init, vtables, COMDAT
#include <string>
#include <stdexcept>
#include <vector>
thread_local std::string tl_s = "abc";
static thread_local int tl_i = 4;
extern thread_local int tl_ext;
struct B { virtual ~B() {} virtual int f() { return 1; } };
struct D : B { int f() override { return 2; } };
template <class T> T twice(T x) { static T cache = x; return cache + x + x; }
int f2(int x) {
  if (x > 10) throw std::runtime_error("x");
  tl_i += x;
  B *b = x ? new D : new B; int r = b->f(); delete b;
  std::vector<int> v(x, 3);
  try { if (x == 7) throw 7; } catch (int e) { r += e; }
  return tl_s.size() + tl_i + tl_ext + r + twice(x) + (int)twice(1.5) + (int)v.size();
}
