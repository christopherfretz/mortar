// WP-250: the C++ header of a.cc b.cc c.cc: macros (GCC -g3's
// .debug_macro groups), a template and an inline function (code COMDAT
// groups whose dropped copies the debug sections also name).
#ifndef WP250_HH
#define WP250_HH
#include <cstddef>
#include <climits>
#define WP250_ANSWER 42
#define WP250_TWICE(x) ((x) * 2)
template <class T> struct Box {
  T v;
  T get() const { return v + WP250_ANSWER; }
};
inline int wp250_twice(int x) { return WP250_TWICE(x); }
int b_f(int);
int c_f(int);
#endif
