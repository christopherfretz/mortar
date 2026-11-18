// WP-249: the C++ header: macros, a struct, a class template and an
// inline function (COMDAT code and, with -fdebug-types-section, type units).
#pragma once
#define WP249_ANSWER 42
#define WP249_TWICE(x) ((x) * 2)
struct Point { int x; long y; };
template <typename T> struct Box {
  T v;
  T get() const { return v + 1; }
};
inline int twice(int x) { return x * 2; }
int b_f(int);
int c_f(int);
extern int g_b;
