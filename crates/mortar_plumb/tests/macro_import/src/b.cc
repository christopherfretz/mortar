#include "h.hh"
int b_f(int x) { Box<int> b{x}; return b.get() + wp250_twice(x) + (int)sizeof(std::size_t); }
