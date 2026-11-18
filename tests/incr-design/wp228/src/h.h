// shared header: a template and an inline function (COMDAT in each user),
// a class with a virtual function (vtable, key function in c.cc)
#pragma once
template <typename T> struct Box {
  T v;
  T get() const { return v + 1; }
};
inline int twice(int x) { return x * 2; }
struct Shape {
  virtual ~Shape();
  virtual int area() const;
  int w = 3;
};
int b_f(int);
int c_f(int);
extern int g_b;
extern const char *g_msg;
