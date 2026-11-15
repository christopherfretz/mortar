#include <cstdio>
#include <string>
#include <vector>
#include <stdexcept>
#include <memory>
thread_local std::string tls_s = "init";
thread_local int tli = 3;
struct S { virtual ~S(){} virtual int f(){ return 1; } };
struct T : S { int f() override { return 2; } };
template<class X> struct C { static X v; static thread_local X tv; };
template<class X> X C<X>::v = X(7);
template<class X> thread_local X C<X>::tv = X(8);
int thrower(int x){ if (x > 0) throw std::runtime_error("boom"); return x; }
int main(int argc, char**){
  std::vector<std::unique_ptr<S>> v; v.emplace_back(new T); v.emplace_back(new S);
  int r = 0; for (auto &p : v) r += p->f();
  try { thrower(argc); } catch (const std::exception &e) { r += 100; tls_s += e.what(); }
  r += C<int>::v + C<int>::tv + C<long>::tv + tli;
  std::printf("%d %s\n", r, tls_s.c_str());
  return 0;
}
