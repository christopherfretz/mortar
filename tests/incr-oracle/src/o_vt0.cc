#include "o_vt.h"
struct D : B { int f(int x) override; };
int D::f(int x) { return x * 2; }
B *make(int) { return new D; }
int lf(int x){ return x; }
