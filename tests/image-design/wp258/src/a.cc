#include "h.hh"
int b_f(long v);
int a_f(long v) { wp258::Box<long> b{v}; return int(b.get()) + wp258::twice(1); }
int main() { return a_f(1) + b_f(2) == 9 ? 0 : 1; }
