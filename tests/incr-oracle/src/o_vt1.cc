#include "o_vt.h"
struct D : B { int f(int x) override; int g(int x) override; virtual int h(int x); };
int D::f(int x) { return x * 2; }
int D::g(int x) { return x * 3; }
int D::h(int x) { return x * 4; }
B *make(int) { return new D; }
int lf(int x){ return x; }
