// C++: exceptions, virtual calls, thread_local, a static map.
#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>
#include <map>
thread_local int tl_a = 7;
thread_local std::string tl_s = "tls";
struct B { virtual ~B() {} virtual int f() const { return 1; } };
struct D : B { int f() const override { return 2; } };
static std::map<std::string,int> gm = {{"a",1},{"b",2}};
int thrower(int x){ if (x > 0) throw std::runtime_error("boom"); return x; }
int main(int argc, char**){
  std::vector<B*> v{new B, new D};
  int s = 0; for (auto *p : v) s += p->f();
  try { thrower(argc); } catch (const std::exception &e) { std::cout << "caught " << e.what() << "\n"; }
  std::cout << "s=" << s << " tl=" << tl_a << tl_s << " m=" << gm["b"] << std::endl;
  return s == 3 ? 0 : 1;
}
