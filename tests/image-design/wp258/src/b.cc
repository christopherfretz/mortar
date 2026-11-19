#include "h.hh"
int b_f(long v) { wp258::Box<long> b{v}; return int(b.get()) + wp258::twice(1); }
