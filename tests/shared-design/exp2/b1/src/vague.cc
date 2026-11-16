// libstdc++-style C++: inline variables, vague linkage, function-local statics,
// typeinfo/vtables, exceptions, an exported global next to static data.
#include <string>
#include <vector>
#include <stdexcept>
#include <map>
struct Base { virtual ~Base(); virtual int f() const; int k = 1; };
Base::~Base() {}
int Base::f() const { return k; }
struct Der : Base { int f() const override { return k + 1; } };
inline int inl_counter = 0;                      // inline variable: vague linkage
template <class T> struct Reg { static T value; static int bump() { return ++value; } };
template <class T> T Reg<T>::value = T();
template struct Reg<int>;
int exported_table[4] = {1, 2, 3, 4};
static int static_table[4] = {5, 6, 7, 8};
int exported_after_static = 9;
const char *const exported_names[] = {"a", "b"};
static const char *const local_names[] = {"c", "d"};
int &singleton() { static int s = 42; return s; }   // STB_GNU_UNIQUE in an inline? (not inline here)
inline int &inl_singleton() { static int s = 43; return s; }
std::string greet(const std::string &w) { return "hello " + w; }
int use_all(int i) {
  ++inl_counter; Reg<int>::bump(); inl_singleton()++;
  Der d; Base &b = d;
  std::map<int, std::string> m{{1, "x"}};
  return exported_table[i & 3] + static_table[i & 3] + exported_after_static + b.f() + singleton() +
         (int)m.size() + (int)greet(local_names[i & 1]).size() + (int)std::string(exported_names[i & 1]).size();
}
void thrower(int v) { if (v) throw std::runtime_error("boom"); }
int catcher(int v) { try { thrower(v); } catch (const std::exception &e) { return 1; } return 0; }
std::vector<int> make_vec(int n) { return std::vector<int>(n, inl_counter); }
int *end_of_static() { return &static_table[4]; }
int *static_end_ptr = &static_table[4];        // one-past-the-end of a local object, as data
