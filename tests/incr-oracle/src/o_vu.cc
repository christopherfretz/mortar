#include "o_vt.h"
B::~B() {}
int B::f(int x) { return x; }
int B::g(int x) { return -x; }
int uf(int x){ B *b = make(x); int r = b->f(x) + b->g(x); delete b; return r; }
